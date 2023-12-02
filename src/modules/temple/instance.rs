use super::{
    structs::{Collection, CollectionAccount},
    utils::{check_collection_validity, wait_until_live},
};
use crate::{
    config::{Settings, Wallets},
    modules::{
        canvas::Canvas,
        temple::utils::{get_collection_config, time_until_live},
    },
    programs,
    tpu_client::TpuClient,
    utils::{self, SolanaWallet, SolanaWallets},
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

pub struct Instance {
    canvas: Canvas,
    settings: Settings,
    collection: Collection,
    wallets: Arc<SolanaWallets>,
    exit: Arc<AtomicBool>,
}

impl Instance {
    pub async fn new(settings: Settings, wallets: Wallets) -> Result<Self> {
        let wallets = Arc::new(utils::convert_wallets(&settings.tasks.wallets, wallets)?);
        let canvas = Canvas::new(settings.general.rpc_api_url.clone());
        let exit = Arc::new(AtomicBool::new(false));

        let collection = get_collection_config(
            settings.tasks.temple.config_key.clone().into(),
            &canvas
                .fetch_account::<CollectionAccount>(
                    &settings.tasks.temple.config_key.clone().into(),
                )
                .await?,
            &canvas,
        )
        .await?;

        info!("fetched collection:\n{:#?}", collection);

        Ok(Instance {
            canvas,
            settings,
            collection,
            wallets,
            exit,
        })
    }

    pub async fn init(&mut self) -> Result<()> {
        // check if oos or release ended
        check_collection_validity(&self.collection)?;

        // check if is live, otherwise wait until live
        wait_until_live(&self.collection, self.settings.tasks.temple.whitelist).await;

        self.start_tasks().await?;
        Ok(())
    }

    async fn start_tasks(&mut self) -> Result<()> {
        // pre-farm shared variables
        let (.., recent_blockhash, transactions_count) = self
            .canvas
            .fetch_transactions_data(Arc::clone(&self.exit))
            .await?;

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

        let whitelist = self.settings.tasks.temple.whitelist;

        for _ in 0..cpus {
            let tpu_client = Arc::clone(&tpu_client);
            let exit = Arc::clone(&self.exit);
            let collection = self.collection.clone();
            let wallets = Arc::clone(&self.wallets);
            let recent_blockhash = Arc::clone(&recent_blockhash);
            let transactions_count = Arc::clone(&transactions_count);

            threads.push(thread::spawn(move || {
                // because we originally started earlier to pre-farm, we wait until release is live
                thread::sleep(time_until_live(&collection, whitelist));

                'main: loop {
                    for wallet in wallets.iter() {
                        if exit.load(Ordering::Relaxed) {
                            break 'main;
                        }

                        let recent_blockhash = recent_blockhash.read().unwrap();

                        if let Err(e) = tpu_client.try_send_transaction(&create_transaction(
                            &collection,
                            wallet,
                            &recent_blockhash,
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
                error!("{e:?}")
            };
        }

        info!("stopped all tasks");

        Ok(())
    }
}

fn create_transaction(
    collection: &Collection,
    wallet: &SolanaWallet,
    recent_blockhash: &Hash,
) -> Transaction {
    let payer = wallet.keypair.pubkey();
    let mint = Keypair::new();

    let ata = get_associated_token_address(&payer, &mint.pubkey());
    let k = Pubkey::find_program_address(
        &[
            &[109, 101, 116, 97, 100, 97, 116, 97],
            programs::TOKEN_METADATA_PROGRAM.as_ref(),
            mint.pubkey().as_ref(),
        ],
        &programs::TOKEN_METADATA_PROGRAM,
    )
    .0;
    let a = Pubkey::find_program_address(
        &[
            &[
                (255 & collection.pda_buf) as u8,
                ((65280 & collection.pda_buf) >> 8) as u8,
            ],
            payer.as_ref(),
            programs::TEMPLE_PROGRAM.as_ref(),
        ],
        &programs::TEMPLE_PROGRAM,
    )
    .0;
    let p = Pubkey::find_program_address(
        &[
            &[108, 116, 105, 109, 101],
            payer.as_ref(),
            programs::TEMPLE_PROGRAM.as_ref(),
        ],
        &programs::TEMPLE_PROGRAM,
    )
    .0;
    let t = Pubkey::find_program_address(
        &[
            &[109, 101, 116, 97, 100, 97, 116, 97],
            programs::TOKEN_METADATA_PROGRAM.as_ref(),
            mint.pubkey().as_ref(),
            &[101, 100, 105, 116, 105, 111, 110],
        ],
        &programs::TOKEN_METADATA_PROGRAM,
    )
    .0;
    let i = Pubkey::find_program_address(
        &[
            payer.as_ref(),
            spl_token::id().as_ref(),
            collection.token_mint_account.as_ref(),
        ],
        &spl_associated_token_account::id(),
    )
    .0;
    let m = Pubkey::find_program_address(
        &[
            &[109, 101, 116, 97, 100, 97, 116, 97],
            programs::TOKEN_METADATA_PROGRAM.as_ref(),
            collection.col_key.as_ref(),
            &[101, 100, 105, 116, 105, 111, 110],
        ],
        &programs::TOKEN_METADATA_PROGRAM,
    )
    .0;
    let d = Pubkey::find_program_address(
        &[
            &[109, 101, 116, 97, 100, 97, 116, 97],
            programs::TOKEN_METADATA_PROGRAM.as_ref(),
            collection.col_key.as_ref(),
        ],
        &programs::TOKEN_METADATA_PROGRAM,
    )
    .0;
    let j = Pubkey::find_program_address(
        &[
            &[
                (255 & collection.pda_buf) as u8,
                ((65280 & collection.pda_buf) >> 8) as u8,
            ],
            &[97, 117, 116, 104],
            programs::TEMPLE_PROGRAM.as_ref(),
        ],
        &programs::TEMPLE_PROGRAM,
    )
    .0;

    Transaction::new_signed_with_payer(
        &[
            Instruction::new_with_bytes(
                programs::COMPUTE_BUDGET_PROGRAM,
                &[0, 48, 87, 5, 0, 0, 0, 0, 0],
                Vec::new(),
            ),
            Instruction::new_with_bytes(
                programs::TEMPLE_PROGRAM,
                &[100],
                vec![
                    AccountMeta::new(payer, true),
                    AccountMeta::new(mint.pubkey(), true),
                    AccountMeta::new(ata, false),
                    AccountMeta::new_readonly(spl_token::id(), false),
                    AccountMeta::new_readonly(spl_associated_token_account::id(), false),
                    AccountMeta::new_readonly(system_program::id(), false),
                    AccountMeta::new_readonly(sysvar::rent::id(), false),
                ],
            ),
            Instruction::new_with_bytes(
                programs::TEMPLE_PROGRAM,
                &[10, 1],
                vec![
                    AccountMeta::new(payer, true),
                    AccountMeta::new(collection.config_key, false),
                    AccountMeta::new(collection.index_key, false),
                    AccountMeta::new_readonly(collection.wl_key, false),
                    AccountMeta::new_readonly(ata, false),
                    AccountMeta::new_readonly(system_program::id(), false),
                    AccountMeta::new(k, false),
                    AccountMeta::new_readonly(mint.pubkey(), false),
                    AccountMeta::new_readonly(programs::TOKEN_METADATA_PROGRAM, false),
                    AccountMeta::new_readonly(sysvar::rent::id(), false),
                    AccountMeta::new_readonly(sysvar::instructions::id(), false),
                    AccountMeta::new_readonly(spl_token::id(), false),
                    AccountMeta::new(a, false),
                    AccountMeta::new(p, false),
                    AccountMeta::new(t, false),
                    AccountMeta::new_readonly(collection.dao_wallet, false),
                    AccountMeta::new_readonly(collection.token_mint_account, false),
                    AccountMeta::new_readonly(i, false),
                    AccountMeta::new_readonly(collection.token_recip_account, false),
                    AccountMeta::new_readonly(collection.col_key, false),
                    AccountMeta::new_readonly(m, false),
                    AccountMeta::new_readonly(d, false),
                    AccountMeta::new(collection.our_wallet, false),
                    AccountMeta::new(collection.primary_wallet, false),
                    AccountMeta::new(j, false),
                ],
            ),
            Instruction::new_with_bytes(programs::TEMPLE_PROGRAM, &[250], Vec::new()),
        ],
        Some(&payer),
        &[&wallet.keypair, &mint],
        *recent_blockhash,
    )
}
