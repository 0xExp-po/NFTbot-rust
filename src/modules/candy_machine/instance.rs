use super::{
    structs::{CandyMachine, WhitelistMintMode},
    utils::*,
};
use crate::{
    config::{Settings, Wallets},
    modules::canvas::Canvas,
    programs,
    tpu_client::TpuClient,
    utils::{self, CandyMachineCondvar, SolanaWallet, SolanaWallets},
};
use anyhow::Result;
use log::{error, info};
use solana_client::{rpc_client::RpcClient, tpu_client::TpuClientConfig};
use solana_sdk::{
    commitment_config::CommitmentConfig,
    hash::Hash,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    system_program, sysvar,
    transaction::Transaction,
};
use spl_associated_token_account::get_associated_token_address;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
};
use tokio::sync::Notify;

pub struct Instance {
    canvas: Canvas,
    settings: Settings,
    wallets: Arc<SolanaWallets>,
    candy_machine: CandyMachineCondvar<CandyMachine>,
    exit: Arc<AtomicBool>,
}

impl Instance {
    pub async fn new(settings: Settings, wallets: Wallets) -> Result<Self> {
        let wallets = Arc::new(utils::convert_wallets(&settings.tasks.wallets, wallets)?);
        let canvas = Canvas::new(settings.general.rpc_api_url.clone());
        let exit = Arc::new(AtomicBool::new(false));

        let candy_machine = canvas
            .fetch_account(&settings.tasks.candy_machine.candy_machine_id.clone().into())
            .await?;

        info!("fetched candy machine:\n{:#?}", candy_machine);

        let candy_machine = Arc::new((tokio::sync::Mutex::new(candy_machine), Notify::new()));

        Ok(Instance {
            canvas,
            settings,
            wallets,
            candy_machine,
            exit,
        })
    }

    pub async fn init(&mut self) -> Result<()> {
        self.canvas.sold_out_monitor(
            self.settings
                .tasks
                .candy_machine
                .candy_machine_id
                .clone()
                .into(),
            Arc::clone(&self.candy_machine),
            check_candy_machine_validity,
            Arc::clone(&self.exit),
        );

        // check if oos or release ended
        {
            let lock = (&*self.candy_machine).0.lock().await;
            check_candy_machine_validity(&*lock)?;
        }

        // check if is live, otherwise wait until live
        wait_until_live(
            Arc::clone(&self.candy_machine),
            self.settings.tasks.candy_machine.whitelist,
        )
        .await?;

        self.start_tasks().await?;

        Ok(())
    }

    async fn start_tasks(&mut self) -> Result<()> {
        // pre-farm shared variables
        let (rent, recent_blockhash, transactions_count) = self
            .canvas
            .fetch_transactions_data(Arc::clone(&self.exit))
            .await?;

        // clone candy machine so Arc<Mutex> doens't have to be used
        let candy_machine;
        {
            let lock = (&*self.candy_machine).0.lock().await;
            candy_machine = (*lock).clone();
        }

        // create n = cpu len as spamming txs exhaust every core
        let cpus = num_cpus::get();
        let mut threads: Vec<JoinHandle<Result<()>>> = Vec::with_capacity(cpus);

        let tpu_client = Arc::new(TpuClient::new(
            Arc::new(RpcClient::new_with_commitment(
                self.settings.general.rpc_api_url.clone(),
                CommitmentConfig::processed(),
            )),
            utils::rpc_to_ws(self.settings.general.rpc_api_url.clone()).as_str(),
            TpuClientConfig::default(),
        )?);

        let whitelist = self.settings.tasks.candy_machine.whitelist;

        for _ in 0..cpus {
            let tpu_client = Arc::clone(&tpu_client);
            let exit = Arc::clone(&self.exit);
            let candy_machine = candy_machine.clone();
            let wallets = Arc::clone(&self.wallets);
            let candy_machine_pub = self
                .settings
                .tasks
                .candy_machine
                .candy_machine_id
                .clone()
                .into();
            let recent_blockhash = Arc::clone(&recent_blockhash);
            let transactions_count = Arc::clone(&transactions_count);

            threads.push(thread::spawn(move || {
                // because we originally started earlier to pre-farm, we wait until release is live
                thread::sleep(time_until_live(&candy_machine, whitelist)?);

                'main: loop {
                    for wallet in wallets.iter() {
                        if exit.load(Ordering::Relaxed) {
                            break 'main;
                        }

                        let recent_blockhash = recent_blockhash.read().unwrap();

                        if let Err(e) = tpu_client.try_send_transaction(&create_transaction(
                            &candy_machine_pub,
                            &candy_machine,
                            wallet,
                            *recent_blockhash,
                            rent,
                        )) {
                            error!("[{}] error sending transaction: {}", wallet.name, e);
                        } else {
                            transactions_count.fetch_add(1, Ordering::Relaxed);
                        };
                    }
                }

                Ok(())
            }));
        }

        for handle in threads {
            if let Err(e) = handle.join() {
                error!("{e:?}");
            };
        }

        info!("stopped all tasks");

        Ok(())
    }
}

fn create_transaction(
    candy_machine_pub: &Pubkey,
    candy_machine: &CandyMachine,
    wallet: &SolanaWallet,
    recent_blockhash: Hash,
    rent: u64,
) -> Transaction {
    let payer = wallet.keypair.pubkey();
    let mint = Keypair::new();

    let user_token_account_address = get_associated_token_address(&payer, &mint.pubkey());
    let user_paying_account_address = if let Some(ref token_mint) = candy_machine.token_mint {
        get_associated_token_address(&payer, token_mint)
    } else {
        payer
    };

    let mut signers = vec![&wallet.keypair, &mint];
    let mut remaining_accounts: Vec<AccountMeta> = Vec::new();

    let mut instructions = utils::create_init_mint_instructions(
        &payer,
        &user_token_account_address,
        &mint.pubkey(),
        rent,
    );

    if let Some(gatekeeper) = &candy_machine.data.gatekeeper {
        remaining_accounts.push(AccountMeta::new(
            get_gateway_ata(&payer, &gatekeeper.gatekeeper_network),
            false,
        ));

        if gatekeeper.expire_on_use {
            remaining_accounts.push(AccountMeta::new_readonly(programs::CIVIC_PROGRAM, false));

            remaining_accounts.push(AccountMeta::new_readonly(
                get_gateway_expiration(&gatekeeper.gatekeeper_network),
                false,
            ))
        }
    }

    let whitelist_burn_authority;

    if let Some(whitelist_mint_settings) = &candy_machine.data.whitelist_mint_settings {
        let whitelist_token = get_associated_token_address(&payer, &whitelist_mint_settings.mint);

        remaining_accounts.push(AccountMeta::new(whitelist_token, false));

        if let WhitelistMintMode::BurnEveryTime = whitelist_mint_settings.mode {
            whitelist_burn_authority = Keypair::new();

            signers.push(&whitelist_burn_authority);

            remaining_accounts.push(AccountMeta::new(whitelist_mint_settings.mint, false));
            remaining_accounts.push(AccountMeta::new_readonly(
                whitelist_burn_authority.pubkey(),
                true,
            ));
        }
    }

    if candy_machine.token_mint.is_some() {
        remaining_accounts.push(AccountMeta::new(user_paying_account_address, false));
        remaining_accounts.push(AccountMeta::new_readonly(payer, true));
    }

    let (metadata, master_edition) = utils::get_metadata_master(&mint.pubkey());
    let (candy_machine_creator, bump) = get_candy_machine_creator(&candy_machine_pub);

    let mut accounts = vec![
        AccountMeta::new(*candy_machine_pub, false),
        AccountMeta::new_readonly(candy_machine_creator, false),
        AccountMeta::new_readonly(payer, true),
        AccountMeta::new(candy_machine.wallet, false),
        AccountMeta::new(metadata, false),
        AccountMeta::new(mint.pubkey(), false),
        AccountMeta::new_readonly(payer, true),
        AccountMeta::new_readonly(payer, true),
        AccountMeta::new(master_edition, false),
        AccountMeta::new_readonly(programs::TOKEN_METADATA_PROGRAM, false),
        AccountMeta::new_readonly(spl_token::id(), false),
        AccountMeta::new_readonly(system_program::id(), false),
        AccountMeta::new_readonly(sysvar::rent::id(), false),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
        #[allow(deprecated)]
        AccountMeta::new_readonly(sysvar::recent_blockhashes::id(), false),
        AccountMeta::new_readonly(sysvar::instructions::id(), false),
    ];

    accounts.append(&mut remaining_accounts);

    instructions.push(Instruction::new_with_bytes(
        programs::CANDY_MACHINE_PROGRAM,
        &[0xd3, 0x39, 0x06, 0xa7, 0x0f, 0xdb, 0x23, 0xfb, bump],
        accounts,
    ));

    Transaction::new_signed_with_payer(&instructions, Some(&payer), &signers, recent_blockhash)
}
