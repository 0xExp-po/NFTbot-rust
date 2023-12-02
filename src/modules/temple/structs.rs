use borsh::BorshDeserialize;
use solana_sdk::pubkey::Pubkey;

use crate::utils::CandyMachineAccount;

#[derive(BorshDeserialize, Debug, Clone)]
pub struct CollectionAccount {
    pub presale_time: u32,
    pub public_time: u32,
    pub presale_price: u64,
    pub price: u64,
    pub our_cut: String,
    pub sname: String,
    pub symbol: String,
    pub per_wallet: u8,
    pub pda_buf: u64,
    pub uri: String,
    pub index_cap: u16,
    pub auth_pda: Pubkey,
    pub index_key: Pubkey,
    pub wl_key: Pubkey,
    pub wl_size: u16,
    pub ctimeout: u16,
    pub config_seed: u64,
    pub primary_wallet: Pubkey,
    pub sfbp: u16,
    pub secondary_wl_index: u16,
    pub collection_key: Pubkey,
    pub creator_1: Pubkey,
    pub creator_1_cut: u8,
    pub collection_name: String,
    pub creator_2: Option<Pubkey>,
    pub creator_2_cut: Option<u8>,
    pub creator_3: Option<Pubkey>,
    pub creator_3_cut: Option<u8>,
    pub creator_4: Option<Pubkey>,
    pub creator_4_cut: Option<u8>,
    pub token_mint_account: Option<Pubkey>,
    pub token_recip_account: Option<Pubkey>,
    pub token_decimals: Option<u8>,
    pub token_price: Option<u64>,
    pub token_option: Option<u8>,
    pub our_wallet: Option<String>,
    pub given_dao: Option<String>,
    pub token_mints: Option<u16>,
}

impl CandyMachineAccount for CollectionAccount {
    fn has_discriminator() -> bool {
        false
    }
}

#[derive(Clone, Debug, Default)]
pub struct Collection {
    pub pda_buf: u64,
    pub primary_wallet: Pubkey,
    pub index_key: Pubkey,
    pub wl_key: Pubkey,
    pub config_key: Pubkey,
    pub dao_wallet: Pubkey,
    pub our_wallet: Pubkey,
    pub token_mint_account: Pubkey,
    pub token_recip_account: Pubkey,
    pub col_key: Pubkey,

    pub items_left: u16,
    pub presale_time: u32,
    pub public_time: u32,
}
