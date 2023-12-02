use super::{structs::*, utils::*};
use crate::{
    config::{Settings, Wallets},
    modules::canvas::Canvas,
    programs,
    tpu_client::TpuClient,
    utils::{
        self, get_master_edition, get_metadata, rpc_to_ws, CandyMachineCondvar, SolanaWallet,
        SolanaWallets,
    },
};
use anyhow::{Error, Result};
use bincode::deserialize;
use hyper::{body::Buf, client::HttpConnector, Body, Client, Request};
use hyper_openssl::HttpsConnector;
use log::{error, info, warn};
use solana_client::{rpc_client::RpcClient, tpu_client::TpuClientConfig};
use solana_sdk::{
    commitment_config::CommitmentConfig, hash::Hash, pubkey, pubkey::Pubkey, signature::Keypair,
    signer::Signer, system_program, sysvar, transaction::Transaction,
};
use spl_associated_token_account::get_associated_token_address;
use std::{
    str::FromStr,
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
    candy_machine_pub: Pubkey,
    candy_machine: CandyMachineCondvar<CandyMachine>,
    launch_stage: LaunchStage,
    exit: Arc<AtomicBool>,
}

const NATIVE_MINT: Pubkey = pubkey!("So11111111111111111111111111111111111111112");

impl Instance {
    pub async fn new(settings: Settings, wallets: Wallets) -> Result<Self> {
        let wallets = Arc::new(utils::convert_wallets(&settings.tasks.wallets, wallets)?);
        let canvas = Canvas::new(settings.general.rpc_api_url.clone());
        let exit = Arc::new(AtomicBool::new(false));

        warn!("fetching collection's public key...");

        let candy_machine_pub =
            fetch_collection_pubkey(&canvas, &settings.tasks.magic_eden_launchpad.collection)
                .await?;

        info!("collection's public key: {}", candy_machine_pub.to_string());
        warn!("fetching candy machine and launch stage...");

        let (candy_machine, launch_stage) = tokio::try_join!(
            canvas.fetch_account::<CandyMachine>(&candy_machine_pub),
            fetch_launch_stage(
                &canvas,
                candy_machine_pub,
                &settings.tasks.magic_eden_launchpad.stage
            )
        )?;

        info!("fetched candy machine and launch stage:\n{candy_machine:#?}\n{launch_stage:#?}");

        let candy_machine = Arc::new((tokio::sync::Mutex::new(candy_machine), Notify::new()));

        Ok(Instance {
            canvas,
            settings,
            wallets,
            candy_machine_pub,
            candy_machine,
            launch_stage,
            exit,
        })
    }

    pub async fn init(&mut self) -> Result<()> {
        self.canvas.sold_out_monitor(
            self.candy_machine_pub.clone(),
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
        wait_until_live(&self.launch_stage).await?;

        self.start_tasks().await?;

        Ok(())
    }

    async fn start_tasks(&mut self) -> Result<()> {
        // pre-farm shared variables
        let (_, recent_blockhash, transactions_count) = self
            .canvas
            .fetch_transactions_data(Arc::clone(&self.exit))
            .await?;

        // clone candy machine so Arc<Mutex> doens't have to be used
        let candy_machine;
        {
            let lock = (&*self.candy_machine).0.lock().await;
            candy_machine = (*lock).clone();
        }

        let mut threads: Vec<JoinHandle<Result<()>>> =
            Vec::with_capacity(self.settings.tasks.magic_eden_launchpad.threads as usize);

        let tpu_client = Arc::new(TpuClient::new(
            Arc::new(RpcClient::new_with_commitment(
                self.settings.general.rpc_api_url.clone(),
                CommitmentConfig::processed(),
            )),
            rpc_to_ws(self.settings.general.rpc_api_url.clone()).as_str(),
            TpuClientConfig::default(),
        )?);

        for _ in 0..self.settings.tasks.magic_eden_launchpad.threads {
            let tpu_client = Arc::clone(&tpu_client);
            let candy_machine_pub = self.candy_machine_pub.clone();
            let candy_machine = candy_machine.clone();
            let launch_stage = self.launch_stage.clone();
            let wallets = Arc::clone(&self.wallets);
            let recent_blockhash = Arc::clone(&recent_blockhash);
            let exit = Arc::clone(&self.exit);
            let transactions_count = Arc::clone(&transactions_count);

            threads.push(thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;

                let http_client = Canvas::create_http_client();

                thread::sleep(time_until_live(&launch_stage)?);

                println!("{}", tpu_client.estimated_current_slot());

                'main: loop {
                    for wallet in wallets.iter() {
                        if exit.load(Ordering::Relaxed) {
                            break 'main;
                        }

                        let recent_blockhash = recent_blockhash.read().unwrap();

                        match create_transaction(
                            candy_machine_pub,
                            &candy_machine,
                            &launch_stage,
                            wallet,
                            *recent_blockhash,
                            &http_client,
                            &runtime,
                        ) {
                            Ok(transaction) => {
                                if let Err(e) = tpu_client.try_send_transaction(&transaction) {
                                    error!("[{}] error sending transaction: {}", wallet.name, e);
                                } else {
                                    transactions_count.fetch_add(1, Ordering::Relaxed);
                                };
                            }
                            Err(e) => error!("[{}] error creating transaction: {}", wallet.name, e),
                        }
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

async fn fetch_collection_pubkey(canvas: &Canvas, collection_slug: &String) -> Result<Pubkey> {
    let res = canvas
        .http_client
        .get(api_endpoint(format!("/launchpads/{}", collection_slug).as_str()).parse()?)
        .await?;

    if res.status().is_client_error() || res.status().is_server_error() {
        return Err(anyhow::anyhow!(
            "request error: {}",
            res.status().to_string()
        ));
    }

    let body = hyper::body::aggregate(res).await?;
    let json: CollectionResponse = serde_json::from_reader(body.reader())?;

    Ok(Pubkey::from_str(
        json.mint
            .candy_machine_id
            .ok_or(Error::msg("candy machine isn't set"))?
            .as_str(),
    )?)
}

async fn fetch_launch_stage(
    canvas: &Canvas,
    candy_machine_pub: Pubkey,
    launch_stage_name: &String,
) -> Result<LaunchStage> {
    let launch_stages_pub = get_launch_stages_info(&candy_machine_pub);

    let (launch_stages, launch_stages_chain) = tokio::try_join!(
        async {
            let res = canvas
                .http_client
                .get(
                    notary_endpoint(
                        format!("/contract/{}", candy_machine_pub.to_string()).as_str(),
                    )
                    .parse()?,
                )
                .await?;

            if res.status().is_client_error() || res.status().is_server_error() {
                return Err(anyhow::anyhow!(
                    "request error: {}",
                    res.status().to_string()
                ));
            }

            let body = hyper::body::aggregate(res).await?;
            let json: LaunchStagesReponse = serde_json::from_reader(body.reader())?;

            Ok(json.stages)
        },
        canvas.fetch_account::<LaunchStagesInfo>(&launch_stages_pub)
    )?;

    let launch_stage_index = launch_stages
        .into_iter()
        .position(|stage| *stage.name == *launch_stage_name)
        .ok_or(Error::msg("stage not found"))?;

    let launch_stage = launch_stages_chain
        .stages
        .get(launch_stage_index)
        .ok_or(Error::msg("launch_stage index out of bounds"))?
        .clone();

    Ok(launch_stage)
}

fn create_transaction(
    candy_machine_pub: Pubkey,
    candy_machine: &CandyMachine,
    launch_stage: &LaunchStage,
    wallet: &SolanaWallet,
    recent_blockhash: Hash,
    client: &Client<HttpsConnector<HttpConnector>>,
    runtime: &tokio::runtime::Runtime,
) -> Result<Transaction> {
    let payer = wallet.keypair.pubkey();

    let mint = loop {
        let mint = Keypair::new();

        let metadata_bump = get_metadata(&mint.pubkey()).1;
        let master_edition_bump = get_master_edition(&mint.pubkey()).1;
        let ata_bump = Pubkey::find_program_address(
            &[
                payer.as_ref(),
                spl_token::id().as_ref(),
                mint.pubkey().as_ref(),
            ],
            &programs::TOKEN_METADATA_PROGRAM,
        )
        .1;

        if metadata_bump == 255 && master_edition_bump == 255 && ata_bump == 255 {
            break mint;
        }
    };

    let user_token_account_address = get_associated_token_address(&payer, &mint.pubkey());
    let (metadata, master_edition) = utils::get_metadata_master(&mint.pubkey());
    let (wallet_limit_info, wallet_limit_info_bump) = get_wallet_limit(&payer, &candy_machine_pub);
    let launch_stages_info = get_launch_stages_info(&candy_machine_pub);
    let pay_to =
        get_associated_token_address(&candy_machine.wallet_authority, &launch_stage.payment_mint);
    let pay_from = if launch_stage.payment_mint == NATIVE_MINT {
        payer
    } else {
        get_associated_token_address(&payer, &launch_stage.payment_mint)
    };

    let body = MintInstructionRequest {
        params: MintInstructionParams {
            wallet_limit_info_bump,
            in_order: false,
            blockhash: recent_blockhash.to_string(),
            needs_notary: {
                if let Some(notary) = candy_machine.notary {
                    notary != system_program::id()
                } else {
                    false
                }
            },
        },
        accounts: MintInstructionAccouts {
            config: candy_machine.config.to_string(),
            candy_machine: candy_machine_pub.to_string(),
            launch_stages_info: launch_stages_info.to_string(),
            candy_machine_wallet_authority: candy_machine.wallet_authority.to_string(),
            mint_receiver: payer.to_string(),
            payer: payer.to_string(),
            pay_to: pay_to.to_string(),
            pay_from: pay_from.to_string(),
            mint: mint.pubkey().to_string(),
            token_ata: user_token_account_address.to_string(),
            metadata: metadata.to_string(),
            master_edition: master_edition.to_string(),
            wallet_limit_info: wallet_limit_info.to_string(),
            token_metadata_program: programs::TOKEN_METADATA_PROGRAM.to_string(),
            token_program: spl_token::id().to_string(),
            system_program: system_program::id().to_string(),
            rent: sysvar::rent::id().to_string(),
            order_info: candy_machine.order_info.to_string(),
            slot_hashes: sysvar::slot_hashes::id().to_string(),
            notary: if let Some(notary) = candy_machine.notary {
                notary.to_string()
            } else {
                Pubkey::default().to_string()
            },
            associated_token_program: spl_associated_token_account::id().to_string(),
        },
    };

    let req = Request::post(notary_endpoint("/mintix"))
        .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/102.0.5005.61 Safari/537.36")
        .header("Content-Type", "application/json")
        .header("Origin", "https://magiceden.io")
        .body(Body::from(serde_json::to_vec(&body)?))?;

    let body = runtime.block_on(async {
        let res = client.request(req).await?;

        if res.status().is_client_error() || res.status().is_server_error() {
            return Err(anyhow::anyhow!(
                "request error: {}",
                res.status().to_string()
            ));
        }

        let body = hyper::body::aggregate(res).await?;

        Ok(body)
    })?;

    let json: MintInstructionResponse = serde_json::from_reader(body.reader())?;

    let mut transaction: Transaction = deserialize(&bs58::decode(json.tx).into_vec()?)?;

    transaction.try_partial_sign_unchecked(
        &[&wallet.keypair, &mint],
        vec![0, 1],
        recent_blockhash,
    )?;

    Ok(transaction)
}
