use super::{
    structs::{Collection, InstructionData, WhitelistConfig},
    utils::{check_collection_validity, get_minting, get_wallet_limit, wait_until_live},
};
use crate::{
    config::{Settings, Wallets},
    modules::{canvas::Canvas, solport::utils::time_until_live},
    programs,
    tpu_client::TpuClient,
    utils::{self, SolanaWallet, SolanaWallets},
};
use anyhow::Result;
use lazy_static::lazy_static;
use log::{error, info};
use regex::Regex;
use sha2::{Digest, Sha256};
use sha3::Keccak256;
use solana_client::{rpc_client::RpcClient, tpu_client::TpuClientConfig};
use solana_sdk::{
    commitment_config::CommitmentConfig,
    hash::Hash,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    system_instruction, system_program, sysvar,
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

lazy_static! {
    static ref INSTRUCTION_REGEX: Regex = Regex::new(".{1,2}").unwrap();
}

/*
    Errors:
    0x10 = Forced whitelist?
    0xb (0x11) = Not enough sol
    0x13 = Whitelist limit reached
*/

// random number between 0 and 10_000
const ENTROPY: u16 = 2107;

pub struct Instance {
    canvas: Canvas,
    settings: Settings,
    get_h: Vec<u8>,
    wallets: Arc<SolanaWallets>,
    collection: Collection,
    whitelist_config: Option<WhitelistConfig>,
    exit: Arc<AtomicBool>,
}

impl Instance {
    pub async fn new(settings: Settings, wallets: Wallets) -> Result<Self> {
        let wallets = Arc::new(utils::convert_wallets(&settings.tasks.wallets, wallets)?);
        let canvas = Canvas::new(settings.general.rpc_api_url.clone());
        let exit = Arc::new(AtomicBool::new(false));

        let collection: Collection = canvas
            .fetch_account(&settings.tasks.solport.collection_id.clone().into())
            .await?;

        info!("fetched collection:\n{:#?}", collection);

        let whitelist_config = if let Some(whitelist) = &collection.whitelist {
            Some(canvas.fetch_account(&whitelist.whitelist_config).await?)
        } else {
            None
        };

        let get_h = get_h(&settings);

        Ok(Instance {
            canvas,
            settings,
            wallets,
            collection,
            whitelist_config,
            get_h,
            exit,
        })
    }

    pub async fn init(&mut self) -> Result<()> {
        // check if oos or release ended
        check_collection_validity(&self.collection, self.settings.tasks.solport.whitelist)?;

        // check if is live, otherwise wait until live
        wait_until_live(&self.collection, self.settings.tasks.solport.whitelist).await?;

        self.start_tasks().await?;

        Ok(())
    }

    async fn start_tasks(&mut self) -> Result<()> {
        // pre-farm shared variables
        let (rent, recent_blockhash, transactions_count) = self
            .canvas
            .fetch_transactions_data(Arc::clone(&self.exit))
            .await?;

        // create n = cpu len as spamming txs exhaust every core
        let cpus = num_cpus::get();
        let mut threads: Vec<JoinHandle<Result<()>>> = Vec::with_capacity(cpus);

        let rpc_client = Arc::new(RpcClient::new_with_commitment(
            self.settings.general.rpc_api_url.clone(),
            CommitmentConfig::processed(),
        ));

        let tpu_client = Arc::new(TpuClient::new(
            Arc::clone(&rpc_client),
            utils::rpc_to_ws(self.settings.general.rpc_api_url.clone()).as_str(),
            TpuClientConfig::default(),
        )?);

        for _ in 0..1 {
            let tpu_client = Arc::clone(&tpu_client);
            let rpc_client = Arc::clone(&rpc_client);
            let exit = Arc::clone(&self.exit);
            let collection = self.collection.clone();
            let wallets = Arc::clone(&self.wallets);
            let settings = self.settings.clone();
            let whitelist_config = self.whitelist_config.clone();
            let get_h = self.get_h.clone();
            let recent_blockhash = Arc::clone(&recent_blockhash);
            let transactions_count = Arc::clone(&transactions_count);

            threads.push(thread::spawn(move || {
                // because we originally started earlier to pre-farm, we wait until release is live
                thread::sleep(time_until_live(
                    &collection,
                    settings.tasks.solport.whitelist,
                )?);

                'main: loop {
                    for wallet in wallets.iter() {
                        if exit.load(Ordering::Relaxed) {
                            break 'main;
                        }

                        let recent_blockhash = recent_blockhash.read().unwrap();

                        match rpc_client.simulate_transaction(&create_transaction(
                            &collection,
                            &settings,
                            &whitelist_config,
                            get_h.clone(),
                            wallet,
                            *recent_blockhash,
                            rent,
                        )?) {
                            Ok(tx) => {
                                if let Some(e) = tx.value.err {
                                    println!("{e} {:#?}", tx.value.logs);
                                }
                            }
                            Err(e) => println!("{e}"),
                        }

                        // if let Err(e) = tpu_client.try_send_transaction(&create_transaction(
                        //     &collection,
                        //     &settings,
                        //     &whitelist_config,
                        //     get_h.clone(),
                        //     wallet,
                        //     *recent_blockhash,
                        //     rent,
                        // )?) {
                        //     error!("[{}] error sending transaction: {}", wallet.name, e);
                        // } else {
                        //     transactions_count.fetch_add(1, Ordering::Relaxed);
                        // };
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

fn get_h(settings: &Settings) -> Vec<u8> {
    let entropy_binary_len = format!("{:b}", ENTROPY).len();
    let collection_pub: Pubkey = settings.tasks.solport.collection_id.clone().into();

    Sha256::digest(
        &[
            // this["s3"] =
            settings.tasks.solport.s3.clone().into(),
            vec![
                if entropy_binary_len >= 8 {
                    ((ENTROPY >> 8) & 255) as u8
                } else {
                    0
                },
                #[allow(unused_comparisons)]
                if entropy_binary_len >= 0 {
                    ((ENTROPY >> 0) & 255) as u8
                } else {
                    0
                },
            ],
            collection_pub.to_bytes().to_vec(),
        ]
        .concat(),
    )
    .to_vec()
}

fn create_transaction(
    collection: &Collection,
    settings: &Settings,
    whitelist_config: &Option<WhitelistConfig>,
    get_h: Vec<u8>,
    wallet: &SolanaWallet,
    recent_blockhash: Hash,
    rent: u64,
) -> Result<Transaction> {
    let payer = wallet.keypair.pubkey();
    let collection_pub = settings.tasks.solport.collection_id.clone().into();
    let program_pub = settings.tasks.solport.program_id.clone().into();

    let first_kp = Keypair::new();
    let mint = Keypair::new();

    let wallet_limit_pub = get_wallet_limit(&payer, &collection_pub, &program_pub);
    let minting_pub = get_minting(&collection_pub, &program_pub);

    let mut instructions = vec![system_instruction::create_account(
        &payer,
        &first_kp.pubkey(),
        rent,
        65,
        &program_pub,
    )];

    instructions.append(&mut utils::create_init_mint_instructions(
        &payer,
        &get_associated_token_address(&payer, &mint.pubkey()),
        &mint.pubkey(),
        rent,
    ));

    let (metadata, master_edition) = utils::get_metadata_master(&mint.pubkey());

    let mut accounts = vec![
        AccountMeta::new(payer, true),
        AccountMeta::new(collection_pub, false),
        AccountMeta::new_readonly(collection.metadata_config, false),
        AccountMeta::new_readonly(spl_token::id(), false),
    ];

    if let Some(whitelist) = &collection.whitelist {
        match whitelist.mode {
            0 => accounts.push(AccountMeta::new_readonly(whitelist.whitelist_config, false)),
            1 => {
                accounts.push(AccountMeta::new(whitelist.token_mint, false));
                accounts.push(AccountMeta::new(
                    get_associated_token_address(&payer, &whitelist.token_mint),
                    false,
                ));
            }
            _ => (),
        }
    }

    accounts.append(&mut vec![
        AccountMeta::new(wallet_limit_pub, false),
        AccountMeta::new(settings.tasks.solport.fee_address.clone().into(), false),
        AccountMeta::new(collection.mint_wallet, false),
        AccountMeta::new_readonly(minting_pub, false),
        AccountMeta::new(mint.pubkey(), true),
        AccountMeta::new(metadata, false),
        AccountMeta::new(master_edition, false),
        AccountMeta::new(first_kp.pubkey(), true),
        AccountMeta::new_readonly(programs::TOKEN_METADATA_PROGRAM, false),
        AccountMeta::new_readonly(system_program::id(), false),
        AccountMeta::new_readonly(sysvar::rent::id(), false),
    ]);

    let (signature, recovery_id) = {
        let get_m = [
            // 2nd
            metadata.to_bytes().to_vec(),
            // 1st
            get_h,
            // this["s4"] =
            settings.tasks.solport.s4.clone().into(),
            // 3d
            mint.to_bytes().to_vec(),
        ]
        .concat();

        let get_s = {
            // above ["keyFromPrivate"]

            // keccak256
            let message = Keccak256::digest(get_m).to_vec();

            libsecp256k1::sign(
                &libsecp256k1::Message::parse_slice(&message)?,
                &settings.tasks.solport.elliptic_key.clone().into(),
            )
        };

        get_s
    };

    let data = InstructionData {
        entropy: ENTROPY,
        whitelist_config_index: if settings.tasks.solport.whitelist {
            whitelist_config.as_ref().and_then(|whitelist_config| {
                whitelist_config
                    .lines
                    .iter()
                    .position(|address| address.address == payer)
                    .and_then(|i| Some(i as u16))
            })
        } else {
            None
        },
        // this["instruction"] =
        instruction: 11,
        data: [signature.r.b32(), signature.s.b32()]
            .concat()
            .try_into()
            .unwrap(),
        id: recovery_id.serialize(),
    };

    instructions.push(Instruction::new_with_borsh(program_pub, &data, accounts));

    Ok(Transaction::new_signed_with_payer(
        &instructions,
        Some(&payer),
        &[&wallet.keypair, &first_kp, &mint],
        recent_blockhash,
    ))
}
