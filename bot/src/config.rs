use anyhow::{anyhow, bail, Context, Result};
use solana_sdk::{pubkey::Pubkey, signature::Keypair};
use std::{env, str::FromStr, sync::Arc, time::Duration};

#[derive(Clone)]
pub struct Config {
    pub rpc_url: String,
    pub keypair: Arc<Keypair>,
    pub pool: Pubkey,
    pub feed_id: u32,
    pub channel: String,
    pub lazer_token: String,
    pub interval: Duration,
    pub program_id: Pubkey,
    pub compute_unit_price_micro_lamports: u64,
    pub compute_unit_limit: u32,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let solana_rpc_url = require("SOLANA_RPC_URL")?;
        let private_key = require("PRIVATE_KEY")?;
        let lazer_token = require("PYTH_LAZER_TOKEN")?;
        let channel = require("PYTH_CHANNEL")?;
        let feed_id_raw = require("PYTH_FEED_ID")?;
        let pool_raw = require("POOL_ID")?;
        let interval_ms_raw = require("INTERVAL_MS")?;

        let interval_ms: u64 = interval_ms_raw
            .parse()
            .with_context(|| format!("INTERVAL_MS must be a non-negative integer, got: {interval_ms_raw}"))?;
        if interval_ms == 0 {
            bail!("INTERVAL_MS must be > 0");
        }

        let feed_id: u32 = feed_id_raw
            .parse()
            .with_context(|| format!("PYTH_FEED_ID must be a u32, got: {feed_id_raw}"))?;

        let pool = Pubkey::from_str(&pool_raw)
            .with_context(|| format!("POOL_ID must be a base58 pubkey, got: {pool_raw}"))?;

        let keypair = Arc::new(decode_keypair(&private_key)?);

        let cluster = env::var("CLUSTER").unwrap_or_else(|_| "mainnet".to_string());
        let rpc_url = env::var("SOLANA_RPC_URL").unwrap();

        let compute_unit_price_micro_lamports: u64 = env::var("PRIORITY_FEE_MICRO_LAMPORTS")
            .ok()
            .map(|s| s.parse())
            .transpose()
            .with_context(|| "PRIORITY_FEE_MICRO_LAMPORTS must be a u64")?
            .unwrap_or(50_000);

        let compute_unit_limit: u32 = env::var("COMPUTE_UNIT_LIMIT")
            .ok()
            .map(|s| s.parse())
            .transpose()
            .with_context(|| "COMPUTE_UNIT_LIMIT must be a u32")?
            .unwrap_or(60_000);

        Ok(Self {
            rpc_url,
            keypair,
            pool,
            feed_id,
            channel,
            lazer_token,
            interval: Duration::from_millis(interval_ms),
            program_id: nomad_amm::ID,
            compute_unit_price_micro_lamports,
            compute_unit_limit,
        })
    }
}

fn require(key: &str) -> Result<String> {
    env::var(key).with_context(|| format!("missing required env var: {key}"))
}

/// `PRIVATE_KEY` is a Phantom-style base58 encoding of the 64-byte secret key.
fn decode_keypair(raw: &str) -> Result<Keypair> {
    let bytes = bs58::decode(raw.trim())
        .into_vec()
        .context("PRIVATE_KEY must be a base58-encoded 64-byte secret key")?;
    Keypair::try_from(bytes.as_slice())
        .map_err(|e| anyhow!("PRIVATE_KEY is not a valid 64-byte keypair: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::signer::Signer;

    #[test]
    fn decodes_base58_keypair() {
        let kp = Keypair::new();
        let secret_bytes = kp.to_bytes();
        let base58 = bs58::encode(secret_bytes).into_string();
        let decoded = decode_keypair(&base58).unwrap();
        assert_eq!(decoded.pubkey(), kp.pubkey());
    }

    #[test]
    fn rejects_garbage_keypair() {
        assert!(decode_keypair("not a keypair at all").is_err());
        // Base58 that decodes to the wrong length.
        assert!(decode_keypair(&bs58::encode([0u8; 32]).into_string()).is_err());
    }
}
