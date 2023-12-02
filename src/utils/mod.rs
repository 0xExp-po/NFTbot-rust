use crate::{config::Wallets, programs};
use anyhow::{Error, Result};
use log::error;
use solana_sdk::{
    instruction::Instruction, program_pack::Pack, pubkey::Pubkey, signature::Keypair,
    system_instruction,
};
use spl_token::state::Mint;
use std::sync::Arc;
use tokio::sync::Notify;

pub trait CandyMachineAccount {
    fn has_discriminator() -> bool;
}

pub type CandyMachineCondvar<T> = Arc<(tokio::sync::Mutex<T>, Notify)>;

pub struct SolanaWallet {
    pub name: String,
    pub keypair: Keypair,
}
pub type SolanaWallets = Vec<SolanaWallet>;

pub fn convert_wallets(wallets_names: &Vec<String>, wallets: Wallets) -> Result<SolanaWallets> {
    if wallets.len() == 0 {
        return Err(Error::msg("no wallets have been selected"));
    }

    let wallets = if wallets_names.get(0).unwrap() == "*" {
        wallets
    } else {
        let mut wallets_names_iter = wallets_names.iter();
        wallets
            .into_iter()
            .filter(|wallet| {
                wallets_names_iter.any(|wallet_name| wallet_name == wallet.name.as_str())
            })
            .collect::<Wallets>()
    };

    let wallets = wallets
        .iter()
        .filter_map(|wallet| {
            let keypair = match bs58::decode(wallet.key.as_str()).into_vec() {
                Ok(bytes) => match Keypair::from_bytes(&bytes) {
                    Ok(k) => k,
                    Err(_) => {
                        error!("wallet: {}'s, private key is invalid", wallet.name);
                        return None;
                    }
                },
                Err(_) => {
                    error!("wallet: {}'s, private key is invalid", wallet.name);
                    return None;
                }
            };

            return Some(SolanaWallet {
                name: wallet.name.clone(),
                keypair,
            });
        })
        .collect::<SolanaWallets>();

    if wallets.len() == 0 {
        return Err(Error::msg("no valid wallets have been selected"));
    }

    Ok(wallets)
}

pub fn create_init_mint_instructions(
    payer: &Pubkey,
    user_token_account_address: &Pubkey,
    mint: &Pubkey,
    rent: u64,
) -> Vec<Instruction> {
    vec![
        system_instruction::create_account(payer, mint, rent, Mint::LEN as u64, &spl_token::id()),
        spl_token::instruction::initialize_mint(&spl_token::id(), mint, payer, Some(payer), 0)
            .unwrap(),
        spl_associated_token_account::instruction::create_associated_token_account(
            payer, payer, mint,
        ),
        spl_token::instruction::mint_to(
            &spl_token::id(),
            mint,
            user_token_account_address,
            payer,
            &[],
            1,
        )
        .unwrap(),
    ]
}

pub fn get_metadata(mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[
            "metadata".as_bytes(),
            programs::TOKEN_METADATA_PROGRAM.as_ref(),
            mint.as_ref(),
        ],
        &programs::TOKEN_METADATA_PROGRAM,
    )
}

pub fn get_master_edition(mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[
            "metadata".as_bytes(),
            programs::TOKEN_METADATA_PROGRAM.as_ref(),
            mint.as_ref(),
            "edition".as_bytes(),
        ],
        &programs::TOKEN_METADATA_PROGRAM,
    )
}

pub fn get_metadata_master(mint: &Pubkey) -> (Pubkey, Pubkey) {
    (get_metadata(mint).0, get_master_edition(mint).0)
}

pub fn rpc_to_ws(rpc_api_url: String) -> String {
    rpc_api_url
        .replace("https://", "wss://")
        .replace("http://", "ws://")
}
