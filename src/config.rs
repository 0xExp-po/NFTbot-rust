use anyhow::Result;
use csv;
use serde::{de::Error, Deserialize, Deserializer};
use solana_sdk::pubkey::Pubkey;
use std::{fs, str::FromStr};
use toml;

#[derive(Debug, Clone)]
pub struct ConfigPubkey(Pubkey);

impl<'de> Deserialize<'de> for ConfigPubkey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let str: &str = Deserialize::deserialize(deserializer)?;
        solana_sdk::pubkey::Pubkey::from_str(str)
            .map(ConfigPubkey)
            .map_err(D::Error::custom)
    }
}

impl Into<Pubkey> for ConfigPubkey {
    fn into(self) -> Pubkey {
        self.0
    }
}

#[derive(Debug, Clone)]
pub struct Base58DecodedString(Vec<u8>);

impl<'de> Deserialize<'de> for Base58DecodedString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let string: String = Deserialize::deserialize(deserializer)?;
        bs58::decode(string)
            .into_vec()
            .map(Base58DecodedString)
            .map_err(D::Error::custom)
    }
}

impl Into<Vec<u8>> for Base58DecodedString {
    fn into(self) -> Vec<u8> {
        self.0
    }
}

#[derive(Debug, Clone)]
pub struct EllipticSecretKey(libsecp256k1::SecretKey);

impl<'de> Deserialize<'de> for EllipticSecretKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let string: String = Deserialize::deserialize(deserializer)?;

        let conversion = || -> Result<libsecp256k1::SecretKey> {
            {
                Ok(libsecp256k1::SecretKey::parse_slice(&hex::decode(
                    String::from_utf8(bs58::decode(string).into_vec()?)?,
                )?)?)
            }
        };

        conversion()
            .map(EllipticSecretKey)
            .map_err(D::Error::custom)
    }
}

impl Into<libsecp256k1::SecretKey> for EllipticSecretKey {
    fn into(self) -> libsecp256k1::SecretKey {
        self.0
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Settings {
    pub general: GeneralSettings,
    pub tasks: Tasks,
}

#[derive(Deserialize, Debug, Clone)]
pub struct GeneralSettings {
    pub rpc_api_url: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Tasks {
    pub wallets: Vec<String>,
    pub candy_machine: CandyMachineSettings,
    pub magic_eden_launchpad: MagicEdenLaunchpadSettings,
    pub solport: SolportSettings,
    pub temple: TempleSettings,
}

#[derive(Deserialize, Debug, Clone)]
pub struct CandyMachineSettings {
    pub candy_machine_id: ConfigPubkey,
    pub whitelist: bool,
}

#[derive(Deserialize, Debug, Clone)]
pub struct MagicEdenLaunchpadSettings {
    pub collection: String,
    pub stage: String,
    pub threads: u64,
}

#[derive(Deserialize, Debug, Clone)]
pub struct SolportSettings {
    pub collection_id: ConfigPubkey,
    pub whitelist: bool,
    pub program_id: ConfigPubkey,
    pub fee_address: ConfigPubkey,
    pub s3: Base58DecodedString,
    pub s4: Base58DecodedString,
    pub elliptic_key: EllipticSecretKey,
}

#[derive(Deserialize, Debug, Clone)]
pub struct TempleSettings {
    pub config_key: ConfigPubkey,
    pub whitelist: bool,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Wallet {
    pub name: String,
    pub key: String,
}

pub type Wallets = Vec<Wallet>;

pub fn read_settings() -> Result<(Settings, Vec<Wallet>)> {
    let settings =
        toml::from_str::<Settings>(&fs::read_to_string("configurations/settings.toml")?)?;
    let wallets = csv::Reader::from_path("configurations/wallets.csv").map(|mut records| {
        records
            .deserialize()
            .map(|record| record.unwrap())
            .collect::<Vec<Wallet>>()
    })?;

    Ok((settings, wallets))
}
