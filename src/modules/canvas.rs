use crate::utils::{self, CandyMachineAccount};
use anyhow::{Error, Result};
use borsh::BorshDeserialize;
use futures_util::StreamExt;
use hyper::{client::HttpConnector, Body, Client};
use hyper_openssl::HttpsConnector;
use log::{error, info, warn};
use openssl::ssl::{SslConnector, SslMethod, SslVerifyMode};
use solana_account_decoder::UiAccountEncoding;
use solana_client::{
    nonblocking::{pubsub_client::PubsubClient, rpc_client::RpcClient},
    rpc_config::RpcAccountInfoConfig,
};
use solana_sdk::{
    account::Account, commitment_config::CommitmentConfig, hash::Hash, pubkey::Pubkey,
};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, RwLock,
    },
    time::Duration,
};
use tokio::sync::Notify;

pub struct Canvas {
    pub rpc_client: RpcClient,
    pub http_client: Client<HttpsConnector<HttpConnector>>,
    pub monitor: Option<tokio::task::JoinHandle<Result<()>>>,
}

impl Canvas {
    pub fn new(rpc_api_url: String) -> Self {
        Canvas {
            rpc_client: RpcClient::new_with_commitment(rpc_api_url, CommitmentConfig::processed()),
            http_client: Self::create_http_client(),
            monitor: None,
        }
    }

    pub async fn fetch_account<T: BorshDeserialize + CandyMachineAccount>(
        &self,
        address: &Pubkey,
    ) -> Result<T> {
        let account = self
            .rpc_client
            .get_account_with_commitment(address, CommitmentConfig::processed())
            .await?
            .value
            .ok_or(Error::msg("account not found"))?;

        let deserialized_account =
            solana_sdk::borsh::try_from_slice_unchecked(if T::has_discriminator() {
                &account.data[8..]
            } else {
                &account.data
            })?;

        Ok(deserialized_account)
    }

    pub fn sold_out_monitor<T, F>(
        &mut self,
        candy_machine_pub: Pubkey,
        candy_machine: Arc<(tokio::sync::Mutex<T>, Notify)>,
        check_candy_machine_validity: F,
        exit: Arc<AtomicBool>,
    ) where
        T: Clone + BorshDeserialize + CandyMachineAccount + Send + 'static,
        F: Fn(&T) -> Result<()> + Send + 'static,
    {
        let rpc_api_url = self.rpc_client.url();

        // spawn thread that updates a candy machine condvar as well as when the release is over
        self.monitor = Some(tokio::spawn(async move {
            let pubsub_client = PubsubClient::new(utils::rpc_to_ws(rpc_api_url).as_str()).await?;

            let (mut receiver, unsubscribe) = pubsub_client
                .account_subscribe(
                    &candy_machine_pub,
                    Some(RpcAccountInfoConfig {
                        commitment: Some(CommitmentConfig::processed()),
                        encoding: Some(UiAccountEncoding::Base64),
                        ..Default::default()
                    }),
                )
                .await?;

            info!("started monitoring: {}", candy_machine_pub);

            while let Some(msg) = receiver.next().await {
                if let Some(account) = msg.value.decode::<Account>() {
                    let new_candy_machine: T =
                        solana_sdk::borsh::try_from_slice_unchecked(if T::has_discriminator() {
                            &account.data[8..]
                        } else {
                            &account.data
                        })?;

                    let (lock, notify) = &*candy_machine;
                    {
                        let mut lock = lock.lock().await;
                        *lock = new_candy_machine.clone();
                    }
                    notify.notify_waiters();

                    // if release is over, update exit variable
                    if let Err(e) = check_candy_machine_validity(&new_candy_machine) {
                        exit.store(true, Ordering::Relaxed);
                        error!("{}", e);
                        break;
                    }
                }
            }

            unsubscribe().await;

            warn!("candy machine monitor stopped");

            Ok(())
        }));
    }

    pub async fn fetch_transactions_data(
        &mut self,
        exit: Arc<AtomicBool>,
    ) -> Result<(u64, Arc<RwLock<Hash>>, Arc<AtomicU64>)> {
        let (rent, (recent_blockhash, _)) = tokio::try_join!(
            self.rpc_client.get_minimum_balance_for_rent_exemption(82),
            self.rpc_client
                .get_latest_blockhash_with_commitment(CommitmentConfig::finalized())
        )?;
        let recent_blockhash = Arc::new(RwLock::new(recent_blockhash));
        // update recent blockhash
        self.recent_blockhash_monitor(Arc::clone(&recent_blockhash), Arc::clone(&exit));

        // count transactions per second
        let transactions_count = Arc::new(AtomicU64::new(0));
        // update transactions count
        self.transactions_rate_monitor(Arc::clone(&transactions_count), Arc::clone(&exit));

        Ok((rent, recent_blockhash, transactions_count))
    }

    pub fn recent_blockhash_monitor(
        &mut self,
        recent_blockhash: Arc<RwLock<Hash>>,
        exit: Arc<AtomicBool>,
    ) {
        let rpc_client = self.create_rpc_client();

        tokio::spawn(async move {
            while !exit.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_secs(30)).await;

                let new_recent_blockhash = match rpc_client
                    .get_latest_blockhash_with_commitment(CommitmentConfig::finalized())
                    .await
                {
                    Ok(b) => b,
                    Err(e) => {
                        error!("error fetching recent blockhash: {}", e);
                        continue;
                    }
                };

                let recent_blockhash = Arc::clone(&recent_blockhash);

                tokio::task::spawn_blocking(move || {
                    *recent_blockhash.write().unwrap() = new_recent_blockhash.0;
                })
                .await?;
            }

            anyhow::Ok(())
        });
    }

    pub fn transactions_rate_monitor(
        &mut self,
        transactions_count: Arc<AtomicU64>,
        exit: Arc<AtomicBool>,
    ) {
        tokio::spawn(async move {
            while !exit.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_secs(1)).await;
                info!(
                    "{} transactions per second",
                    transactions_count.swap(0, Ordering::Relaxed)
                );
            }
        });
    }

    pub fn create_rpc_client(&mut self) -> RpcClient {
        RpcClient::new_with_commitment(self.rpc_client.url(), CommitmentConfig::processed())
    }

    pub fn create_http_client() -> Client<HttpsConnector<HttpConnector>> {
        let mut http_connector = HttpConnector::new();
        http_connector.enforce_http(false);
        http_connector.set_keepalive(Some(Duration::from_secs(20)));
        http_connector.set_nodelay(true);

        let mut ssl_connector = SslConnector::builder(SslMethod::tls_client()).unwrap();
        ssl_connector
            .set_alpn_protos(b"\x02h2\x08http/1.1")
            .unwrap();
        ssl_connector.set_verify(SslVerifyMode::NONE);

        Client::builder().build::<_, Body>(
            HttpsConnector::with_connector(http_connector, ssl_connector).unwrap(),
        )
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        if let Some(monitor) = &self.monitor {
            monitor.abort();
        }
    }
}
