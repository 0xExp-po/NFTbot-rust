use crate::utils::CandyMachineAccount;
use borsh::{BorshDeserialize, BorshSerialize};
use solana_sdk::pubkey::Pubkey;

#[derive(BorshSerialize, Clone, Debug)]
pub struct InstructionData {
    pub instruction: u8,
    pub entropy: u16,
    pub data: [u8; 64],
    pub id: u8,
    pub whitelist_config_index: Option<u16>,
}

#[derive(BorshDeserialize, Clone, Debug)]
pub struct WhitelistConfig {
    pub key: u8,
    pub authority: Pubkey,
    pub max_lines: u16,
    pub position: u16,
    pub lines: Vec<WhitelistAddress>,
}

#[derive(BorshDeserialize, Clone, Debug)]
pub struct WhitelistAddress {
    pub address: Pubkey,
    pub wallet_limit: u8,
}

impl CandyMachineAccount for WhitelistConfig {
    fn has_discriminator() -> bool {
        false
    }
}

#[derive(BorshDeserialize, Clone, Debug)]
pub struct Collection {
    pub key: u8,
    pub authority: Pubkey,
    pub metadata_config: Pubkey,
    pub mint_wallet: Pubkey,
    pub mint_fee_basis_points: u16,
    pub whitelist: Option<Whitelist>,
    pub presale: Option<Presale>,
    pub release_datetime: u64,
    pub price_structure: Vec<Pricing>,
    pub wallet_limit: u16,
    pub settings: Settings,
    pub minted: u16,
    pub total_supply: u16,
    pub closed: bool,
}

impl CandyMachineAccount for Collection {
    fn has_discriminator() -> bool {
        false
    }
}

#[derive(BorshDeserialize, Clone, Debug)]
pub struct Whitelist {
    pub mode: u8,
    pub whitelist_config: Pubkey,
    pub token_mint: Pubkey,
    pub burn: bool,
    pub presale_only: bool,
    pub discounted_price: Option<u64>,
}

#[derive(BorshDeserialize, Clone, Debug)]
pub struct Presale {
    pub start_datetime: u64,
    pub end_datetime: Option<u64>,
    pub supply: u16,
}

#[derive(BorshDeserialize, Clone, Debug)]
pub struct Pricing {
    pub progress: u16,
    pub price: u64,
}

#[derive(BorshDeserialize, Clone, Debug)]
pub struct Settings {
    pub pre_minting: Option<u16>,
    pub bypass_price: bool,
    pub hold_mint_fees: bool,
}
