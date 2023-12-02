use super::structs::{CandyMachine, LaunchStage};
use crate::programs;
use anyhow::{Error, Result};
use chrono::{TimeZone, Utc};
use log::info;
use solana_sdk::pubkey::Pubkey;
use std::time::Duration;

pub fn api_endpoint(endpoint: &str) -> String {
    format!("https://api-mainnet.magiceden.io{}", endpoint)
}

pub fn notary_endpoint(endpoint: &str) -> String {
    format!("https://wk-notary-prod.magiceden.io{}", endpoint)
}

pub fn check_candy_machine_validity(candy_machine: &CandyMachine) -> Result<()> {
    if candy_machine
        .items_available
        .saturating_sub(candy_machine.items_redeemed_normal)
        == 0
    {
        return Err(Error::msg("candy machine is empty"));
    }

    Ok(())
}

pub async fn wait_until_live(launch_stage: &LaunchStage) -> Result<()> {
    let unix = Utc::now().timestamp() as u64;
    let end_time = launch_stage.end_time as u64;
    let start_time = launch_stage.start_time as u64;

    if end_time < unix {
        return Err(Error::msg("stage is concluded"));
    }

    let time_until_live = start_time.saturating_sub(unix);

    if time_until_live > 0 {
        info!(
            "candy machine will start at {}. Waiting...",
            Utc.timestamp(start_time as i64, 0)
                .format("%H:%M:%S")
                .to_string()
        );

        // sleep 5 seconds less so tasks have time to pre-farm
        tokio::time::sleep(Duration::from_secs(time_until_live.saturating_sub(5))).await;
    }

    info!("candy machine is live");

    Ok(())
}

pub fn time_until_live(launch_stage: &LaunchStage) -> Result<Duration> {
    let unix = Utc::now().timestamp() as u64;
    let end_time = launch_stage.end_time as u64;
    let start_time = launch_stage.start_time as u64;

    if end_time < unix {
        return Err(Error::msg("stage is concluded"));
    }

    Ok(Duration::from_secs(start_time.saturating_sub(unix)))
}

pub fn get_launch_stages_info(candy_machine_pub: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            "candy_machine".as_bytes(),
            "launch_stages".as_bytes(),
            candy_machine_pub.as_ref(),
        ],
        &programs::MAGIC_EDEN_LAUNCHPAD_PROGRAM,
    )
    .0
}

pub fn get_wallet_limit(payer: &Pubkey, candy_machine_pub: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[
            "wallet_limit".as_bytes(),
            candy_machine_pub.as_ref(),
            payer.as_ref(),
        ],
        &programs::MAGIC_EDEN_LAUNCHPAD_PROGRAM,
    )
}
