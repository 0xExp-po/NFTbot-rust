use super::structs::{Collection, CollectionAccount};
use crate::modules::canvas::Canvas;
use anyhow::{Error, Result};
use chrono::{TimeZone, Utc};
use log::info;
use solana_sdk::{commitment_config::CommitmentConfig, pubkey::Pubkey};
use std::{str::FromStr, time::Duration};

const DAO_WALLET: Pubkey = solana_sdk::pubkey!("7FHzVCP9eX6zmZjw3qwvmdDMhSvCkLxipQatAqhtbVBf");
const OUR_WALLET: Pubkey = solana_sdk::pubkey!("mnKzuL9RMtR6GeSHBfDpnQaefcMsiw7waoTSduKNiXM");
const TOKEN_MINT_ACCOUNT: Pubkey =
    solana_sdk::pubkey!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");
const TOKEN_RECIP_ACCOUNT: Pubkey =
    solana_sdk::pubkey!("Gdq32GtxXRr9t3BScA6VdtKZ7TFu62d6HBhrNFMZNto9");

pub async fn get_collection_config(
    config_key: Pubkey,
    collection_account: &CollectionAccount,
    canvas: &Canvas,
) -> Result<Collection> {
    let index_account_data = canvas
        .rpc_client
        .get_account_with_commitment(&collection_account.index_key, CommitmentConfig::processed())
        .await?
        .value
        .ok_or(Error::msg("account not found"))?
        .data;

    Ok(Collection {
        pda_buf: collection_account.pda_buf,
        primary_wallet: collection_account.primary_wallet,
        index_key: collection_account.index_key,
        wl_key: collection_account.wl_key,
        config_key,
        dao_wallet: DAO_WALLET,
        our_wallet: collection_account
            .our_wallet
            .as_ref()
            .and_then(|wallet| Some(Pubkey::from_str(wallet.as_str()).unwrap()))
            .or(Some(OUR_WALLET))
            .unwrap(),
        token_mint_account: collection_account
            .token_mint_account
            .or(Some(TOKEN_MINT_ACCOUNT))
            .unwrap(),
        token_recip_account: collection_account
            .token_recip_account
            .or(Some(TOKEN_RECIP_ACCOUNT))
            .unwrap(),
        col_key: collection_account.collection_key,
        items_left: collection_account.index_cap - ((index_account_data[1] as u16) << (8 as u16))
            + index_account_data[0] as u16,
        presale_time: collection_account.presale_time,
        public_time: collection_account.public_time,
    })
}

pub fn check_collection_validity(collection: &Collection) -> Result<()> {
    if collection.items_left == 0 {
        return Err(Error::msg("collection is sold out"));
    }

    Ok(())
}

pub async fn wait_until_live(collection: &Collection, whitelist: bool) {
    let time_until_live;
    let unix = Utc::now().timestamp() as u32;

    if whitelist {
        time_until_live = collection.presale_time.saturating_sub(unix);
    } else {
        time_until_live = collection.public_time.saturating_sub(unix);
    }

    if time_until_live > 0 {
        info!(
            "release will start at {}. Waiting...",
            Utc.timestamp((unix + time_until_live) as i64, 0)
                .format("%H:%M:%S")
                .to_string()
        );

        // sleep 5 seconds less so tasks have time to pre-farm
        tokio::time::sleep(Duration::from_secs(time_until_live.saturating_sub(5) as u64)).await;
    }
}

pub fn time_until_live(collection: &Collection, whitelist: bool) -> Duration {
    let time_until_live;
    let unix = Utc::now().timestamp() as u32;

    if whitelist {
        time_until_live = collection.presale_time.saturating_sub(unix);
    } else {
        time_until_live = collection.public_time.saturating_sub(unix);
    }

    Duration::from_secs(time_until_live as u64)
}
