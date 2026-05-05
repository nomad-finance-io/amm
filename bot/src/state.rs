use anyhow::{bail, Context, Result};
use nomad_amm::curve::fees::FEE_RATE_DENOMINATOR_VALUE;
use solana_sdk::pubkey::Pubkey;

#[derive(Debug, Clone, Copy)]
pub struct PoolCache {
    pub min_spread_bps: u16,
    pub base_trade_fee_rate: u64,
}

/// Fetch `PoolState` once at startup, validate that this bot is the right
/// keeper for it, and pull the pool/config values we will reuse on every tick.
///
/// Validations:
/// - `pool_state.oracle_keeper == signer` — otherwise our pushes will fail
///   on-chain with `InvalidOracleKeeper`, costing us lamports for nothing.
/// - `pool_state.pyth_price_feed_id == feed_id` — otherwise we would push
///   a price for the wrong asset.
pub fn load_blocking(
    rpc_url: &str,
    pool: Pubkey,
    signer: Pubkey,
    feed_id: u32,
) -> Result<PoolCache> {
    use solana_client::rpc_client::RpcClient;
    use solana_sdk::commitment_config::CommitmentConfig;

    let rpc = RpcClient::new_with_commitment(rpc_url.to_string(), CommitmentConfig::confirmed());
    let data = rpc
        .get_account_data(&pool)
        .with_context(|| format!("failed to fetch pool account {pool}"))?;

    let pool_state = decode_pool_state(&data)?;

    // Copy fields out of the packed struct before referencing them — direct
    // references into a `repr(C, packed)` struct are UB when misaligned.
    let amm_config = pool_state.amm_config;
    let pool_oracle_keeper = pool_state.oracle_keeper;
    let pool_feed_id = pool_state.pyth_price_feed_id;
    let min_spread_bps = pool_state.min_spread_bps;

    if pool_oracle_keeper != signer {
        bail!("signer {signer} is not the pool's oracle_keeper ({pool_oracle_keeper})");
    }

    if pool_feed_id != feed_id {
        bail!(
            "PYTH_FEED_ID env ({feed_id}) does not match pool's pyth_price_feed_id ({pool_feed_id})"
        );
    }

    if min_spread_bps == 0 {
        bail!("pool has min_spread_bps = 0; refusing to push (would quote at mid)");
    }

    let amm_config_data = rpc
        .get_account_data(&amm_config)
        .with_context(|| format!("failed to fetch amm_config account {amm_config}"))?;
    let amm_config_state = decode_amm_config(&amm_config_data)?;
    let base_trade_fee_rate = amm_config_state.trade_fee_rate;

    if base_trade_fee_rate >= FEE_RATE_DENOMINATOR_VALUE {
        bail!(
            "amm_config.trade_fee_rate ({base_trade_fee_rate}) must be < {FEE_RATE_DENOMINATOR_VALUE}"
        );
    }

    Ok(PoolCache {
        min_spread_bps,
        base_trade_fee_rate,
    })
}

fn decode_pool_state(data: &[u8]) -> Result<nomad_amm::states::PoolState> {
    use anchor_lang::AccountDeserialize;
    let mut slice: &[u8] = data;
    nomad_amm::states::PoolState::try_deserialize(&mut slice)
        .context("pool account data did not deserialize into PoolState")
}

fn decode_amm_config(data: &[u8]) -> Result<nomad_amm::states::AmmConfig> {
    use anchor_lang::AccountDeserialize;
    let mut slice: &[u8] = data;
    nomad_amm::states::AmmConfig::try_deserialize(&mut slice)
        .context("amm_config account data did not deserialize into AmmConfig")
}
