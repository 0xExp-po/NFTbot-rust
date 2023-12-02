use crate::utils::CandyMachineAccount;
use borsh::BorshDeserialize;
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

#[derive(Deserialize)]
pub struct CollectionResponse {
    pub mint: CollectionResponseMint,
}

#[derive(Deserialize)]
pub struct CollectionResponseMint {
    #[serde(rename = "candyMachineId")]
    pub candy_machine_id: Option<String>,
}

#[derive(Deserialize)]
pub struct LaunchStagesReponse {
    pub stages: Vec<LaunchStagesResponseInfo>,
}

#[derive(Deserialize)]
pub struct LaunchStagesResponseInfo {
    pub name: String,
    #[serde(rename = "startTime")]
    pub start_time: String,
    #[serde(rename = "endTime")]
    pub end_time: String,
}

#[derive(Serialize)]
pub struct MintInstructionRequest {
    pub params: MintInstructionParams,
    pub accounts: MintInstructionAccouts,
}

#[derive(Serialize)]
pub struct MintInstructionParams {
    #[serde(rename = "walletLimitInfoBump")]
    pub wallet_limit_info_bump: u8,
    #[serde(rename = "inOrder")]
    pub in_order: bool,
    pub blockhash: String,
    #[serde(rename = "needsNotary")]
    pub needs_notary: bool,
}

#[derive(Serialize)]
pub struct MintInstructionAccouts {
    pub config: String,
    #[serde(rename = "candyMachine")]
    pub candy_machine: String,
    #[serde(rename = "launchStagesInfo")]
    pub launch_stages_info: String,
    #[serde(rename = "candyMachineWalletAuthority")]
    pub candy_machine_wallet_authority: String,
    #[serde(rename = "mintReceiver")]
    pub mint_receiver: String,
    pub payer: String,
    #[serde(rename = "payTo")]
    pub pay_to: String,
    #[serde(rename = "payFrom")]
    pub pay_from: String,
    pub mint: String,
    #[serde(rename = "tokenAta")]
    pub token_ata: String,
    pub metadata: String,
    #[serde(rename = "masterEdition")]
    pub master_edition: String,
    #[serde(rename = "walletLimitInfo")]
    pub wallet_limit_info: String,
    #[serde(rename = "tokenMetadataProgram")]
    pub token_metadata_program: String,
    #[serde(rename = "tokenProgram")]
    pub token_program: String,
    #[serde(rename = "systemProgram")]
    pub system_program: String,
    pub rent: String,
    #[serde(rename = "orderInfo")]
    pub order_info: String,
    #[serde(rename = "slotHashes")]
    pub slot_hashes: String,
    pub notary: String,
    #[serde(rename = "associatedTokenProgram")]
    pub associated_token_program: String,
}

#[derive(Deserialize)]
pub struct MintInstructionResponse {
    pub tx: String,
}

#[derive(BorshDeserialize, Debug, Clone)]
pub struct CandyMachine {
    pub authority: Pubkey,
    pub wallet_authority: Pubkey,
    pub config: Pubkey,
    pub items_redeemed_normal: u64,
    pub items_redeemed_raffle: u64,
    pub raffle_tickets_purchased: u64,
    pub uuid: String,
    pub items_available: u64,
    pub raffle_seed: u64,
    pub bump: u8,
    pub notary: Option<Pubkey>,
    pub order_info: Pubkey,
}

impl CandyMachineAccount for CandyMachine {
    fn has_discriminator() -> bool {
        true
    }
}

#[derive(BorshDeserialize, Debug, Clone)]
pub struct LaunchStagesInfo {
    pub bump: u8,
    pub authority: Pubkey,
    pub candy_machine: Pubkey,
    pub stages: Vec<LaunchStage>,
}

impl CandyMachineAccount for LaunchStagesInfo {
    fn has_discriminator() -> bool {
        true
    }
}

#[derive(BorshDeserialize, Debug, Clone)]
pub struct LaunchStage {
    pub stage_type: LaunchStageType,
    pub start_time: i64,
    pub end_time: i64,
    pub wallet_limit: WalletLimitSpecification,
    pub price: u64,
    pub stage_supply: Option<u32>,
    pub previous_stage_unminted_supply: u32,
    pub minted_during_stage: u32,
    pub payment_mint: Pubkey,
    pub payment_ata: Pubkey,
}

#[derive(BorshDeserialize, Debug, Clone)]
pub enum LaunchStageType {
    Invalid,
    NormalSale,
    Raffle,
}

#[derive(BorshDeserialize, Debug, Clone)]
pub enum WalletLimitSpecification {
    NoLimit,
    FixedLimit(u8),
    VariableLimit,
}
