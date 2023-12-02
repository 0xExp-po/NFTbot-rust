use std::time::Duration;

use super::structs::Collection;
use anyhow::{Error, Result};
use chrono::{TimeZone, Utc};
use log::info;
use solana_sdk::pubkey::Pubkey;

pub fn check_collection_validity(collection: &Collection, whitelist: bool) -> Result<()> {
    let supply = if whitelist {
        collection
            .presale
            .as_ref()
            .and_then(|presale| Some(presale.supply))
            .or(Some(collection.total_supply))
            .unwrap()
    } else {
        collection.total_supply
    };

    if supply.saturating_sub(collection.minted) == 0 {
        return Err(Error::msg("collection is sold out"));
    }

    if collection.closed {
        return Err(Error::msg("collection is closed"));
    }

    Ok(())
}

pub async fn wait_until_live(collection: &Collection, whitelist: bool) -> Result<()> {
    let mut time_until_live = 0;
    let unix = Utc::now().timestamp() as u64;

    if whitelist {
        if let Some(presale) = &collection.presale {
            // check if presale is over
            if let Some(end_datetime) = presale.end_datetime {
                if end_datetime < unix {
                    return Err(Error::msg("presale is concluded"));
                }
            }

            time_until_live = presale.start_datetime.saturating_sub(unix);
        }
    } else {
        time_until_live = collection.release_datetime.saturating_sub(unix);
    }

    if time_until_live > 0 {
        info!(
            "release will start at {}. Waiting...",
            Utc.timestamp((unix + time_until_live) as i64, 0)
                .format("%H:%M:%S")
                .to_string()
        );

        // sleep 5 seconds less so tasks have time to pre-farm
        tokio::time::sleep(Duration::from_secs(time_until_live.saturating_sub(5))).await;
    }

    Ok(())
}

pub fn time_until_live(collection: &Collection, whitelist: bool) -> Result<Duration> {
    let mut time_until_live = 0;
    let unix = Utc::now().timestamp() as u64;

    if whitelist {
        if let Some(presale) = &collection.presale {
            // check if presale is over
            if let Some(end_datetime) = presale.end_datetime {
                if end_datetime < unix {
                    return Err(Error::msg("presale is concluded"));
                }
            }

            time_until_live = presale.start_datetime.saturating_sub(unix);
        }
    } else {
        time_until_live = collection.release_datetime.saturating_sub(unix);
    }

    Ok(Duration::from_secs(time_until_live))
}

pub fn get_wallet_limit(payer: &Pubkey, collection_pub: &Pubkey, program_pub: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            "wallet_limit".as_bytes(),
            collection_pub.as_ref(),
            payer.as_ref(),
        ],
        program_pub,
    )
    .0
}

pub fn get_minting(collection_pub: &Pubkey, program_pub: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &["minting".as_bytes(), collection_pub.as_ref()],
        program_pub,
    )
    .0
}
