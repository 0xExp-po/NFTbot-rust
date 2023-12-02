use super::structs::{CandyMachine, EndSettingType};
use crate::{programs, utils::CandyMachineCondvar};
use anyhow::{Error, Result};
use chrono::{TimeZone, Utc};
use log::{info, warn};
use solana_sdk::pubkey::Pubkey;
use std::time::Duration;

pub fn check_candy_machine_validity(candy_machine: &CandyMachine) -> Result<()> {
    if candy_machine
        .data
        .items_available
        .saturating_sub(candy_machine.items_redeemed)
        == 0
    {
        return Err(Error::msg("candy machine is empty"));
    }

    if let Some(end_settings) = &candy_machine.data.end_settings {
        match end_settings.end_setting_type {
            EndSettingType::Amount => {
                if candy_machine.items_redeemed >= end_settings.number {
                    return Err(Error::msg("candy machine is empty (end_settings)"));
                }
            }
            EndSettingType::Date => {
                if Utc::now().timestamp() > end_settings.number as i64 {
                    return Err(Error::msg("candy machine stopped (end_settings)"));
                }
            }
        }
    }

    Ok(())
}

pub async fn wait_until_live(
    candy_machine_cvar: CandyMachineCondvar<CandyMachine>,
    whitelist: bool,
) -> Result<()> {
    let mut time_until_live = 0;
    let unix = Utc::now().timestamp() as u64;

    let (lock, notify) = &*candy_machine_cvar;
    let candy_machine = lock.lock().await;

    if let Some(ref whitelist_mint_settings) = &(*candy_machine).data.whitelist_mint_settings {
        if whitelist {
            match candy_machine.data.go_live_date {
                None => {
                    if !whitelist_mint_settings.presale {
                        return Err(Error::msg("go live date is none while presale is disabled"));
                    }
                }
                Some(val) => {
                    if !whitelist_mint_settings.presale {
                        time_until_live = (val as u64).saturating_sub(unix);
                    }
                }
            }
        } else {
            // metaplex bug. Even if go live date is live, if this is true, non-whitelisted accounts will get blocked.
            // To avoid error tax due to metaplex's mistake, the candy machine should be monitored to detect when a
            // valid configuration is set
            if whitelist_mint_settings.discount_price.is_none() && !whitelist_mint_settings.presale
            {
                warn!(
                    "candy machine configuration is bugged. Waiting for a valid configuration..."
                );

                drop(candy_machine);

                // wait until a non-bugged candy machine is set
                while candy_machine_is_bugged(&*lock.lock().await) {
                    notify.notified().await;
                }
                return Ok(());
            }

            match candy_machine.data.go_live_date {
                None => return Err(Error::msg("go live date is none")),
                Some(val) => {
                    time_until_live = (val as u64).saturating_sub(unix);
                }
            }
        }
    } else {
        match candy_machine.data.go_live_date {
            None => return Err(Error::msg("go live date is none")),
            Some(val) => {
                time_until_live = (val as u64).saturating_sub(unix);
            }
        }
    }

    if time_until_live > 0 {
        info!(
            "candy machine will start at {}. Waiting...",
            Utc.timestamp(candy_machine.data.go_live_date.unwrap(), 0)
                .format("%H:%M:%S")
                .to_string()
        );

        drop(candy_machine);

        // sleep 5 seconds less so tasks have time to pre-farm
        tokio::time::sleep(Duration::from_secs(time_until_live.saturating_sub(5))).await;
    }

    info!("candy machine is live");

    Ok(())
}

pub fn time_until_live(candy_machine: &CandyMachine, whitelist: bool) -> Result<Duration> {
    let mut time_until_live = 0;
    let unix = Utc::now().timestamp() as u64;

    if let Some(whitelist_mint_settings) = &candy_machine.data.whitelist_mint_settings {
        if whitelist {
            match candy_machine.data.go_live_date {
                None => {
                    if !whitelist_mint_settings.presale {
                        return Err(Error::msg("go live date is none while presale is disabled"));
                    }
                }
                Some(val) => {
                    if !whitelist_mint_settings.presale {
                        time_until_live = (val as u64).saturating_sub(unix)
                    }
                }
            }
        } else {
            // metaplex bug. Even if go live date is live, if this is true, non-whitelisted accounts will get blocked.
            if whitelist_mint_settings.discount_price.is_none() && !whitelist_mint_settings.presale
            {
                return Err(Error::msg("candy machine configuration is bugged"));
            }

            match candy_machine.data.go_live_date {
                None => return Err(Error::msg("go live date is none")),
                Some(val) => time_until_live = (val as u64).saturating_sub(unix),
            }
        }
    } else {
        match candy_machine.data.go_live_date {
            None => return Err(Error::msg("go live date is none")),
            Some(val) => time_until_live = (val as u64).saturating_sub(unix),
        }
    }

    Ok(Duration::from_secs(time_until_live))
}

pub fn candy_machine_is_bugged(candy_machine: &CandyMachine) -> bool {
    if let Some(whitelist_mint_settings) = &candy_machine.data.whitelist_mint_settings {
        return whitelist_mint_settings.discount_price.is_none()
            && !whitelist_mint_settings.presale;
    }

    false
}

pub fn get_gateway_ata(payer: &Pubkey, gatekeeper_network: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            payer.as_ref(),
            "gateway".as_bytes(),
            &[0, 0, 0, 0, 0, 0, 0, 0],
            gatekeeper_network.as_ref(),
        ],
        &programs::CIVIC_PROGRAM,
    )
    .0
}

pub fn get_gateway_expiration(gatekeeper_network: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[gatekeeper_network.as_ref(), "expire".as_bytes()],
        &programs::CIVIC_PROGRAM,
    )
    .0
}

pub fn get_candy_machine_creator(candy_machine_pub: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &["candy_machine".as_bytes(), candy_machine_pub.as_ref()],
        &programs::CANDY_MACHINE_PROGRAM,
    )
}
