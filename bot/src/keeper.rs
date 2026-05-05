//! Per-interval orchestration: read latest Lazer quote → widen spread by
//! confidence → compute effective bid/ask + dynamic fee → push
//! `update_pool_oracle`.

use anyhow::{anyhow, Result};
use nomad_amm::curve::fees::FEE_RATE_DENOMINATOR_VALUE;
use solana_sdk::signature::Signature;
use tokio::sync::watch;

use crate::config::Config;
use crate::pusher::{self, PushError};
use crate::pyth::LatestQuote;
use crate::state::PoolCache;

#[derive(Debug, thiserror::Error)]
pub enum TickError {
    /// No Lazer quote has been delivered yet (or the latest was rejected by
    /// the oracle math). Caller logs at warn and waits for the next tick.
    #[error("transient: {0}")]
    Transient(String),
    /// On-chain rejected the push with a code that means the pushed values
    /// are invalid. Operator must investigate; retrying will not help.
    #[error("fatal: {0}")]
    Fatal(String),
}

pub async fn tick(
    cfg: &Config,
    cache: &PoolCache,
    quote_rx: &watch::Receiver<Option<LatestQuote>>,
) -> Result<Signature, TickError> {
    let quote = quote_rx
        .borrow()
        .ok_or_else(|| TickError::Transient("no Lazer quote received yet".to_string()))?;

    let (effective_bid, effective_ask, dynamic_fee_rate) = compute_update_fields(quote, *cache)
        .map_err(|e| TickError::Transient(format!("oracle math rejected quote: {e}")))?;

    pusher::send(
        cfg,
        effective_bid,
        effective_ask,
        quote.exponent,
        dynamic_fee_rate,
    )
    .await
    .map_err(|e| match e {
        PushError::Fatal(s) => TickError::Fatal(s),
        PushError::Transient(s) => TickError::Transient(s),
    })
}

/// Compute the effective bid/ask mantissas to push, given a fresh Lazer quote
/// and the pool / config values cached at startup.
fn compute_update_fields(quote: LatestQuote, cache: PoolCache) -> Result<(i64, i64, u64)> {
    let spread_bps = client::oracle_math::compute_confidence_adjusted_min_spread_bps(
        cache.min_spread_bps,
        quote.confidence_mantissa,
        quote.price_mantissa,
    );
    let (effective_bid, effective_ask) = client::oracle_math::compute_effective_bid_ask_mantissas(
        quote.price_mantissa,
        quote.best_bid_mantissa,
        quote.best_ask_mantissa,
        spread_bps,
    )
    .ok_or_else(|| {
        anyhow!("compute_effective_bid_ask_mantissas returned None for quote {quote:?}")
    })?;
    let dynamic_fee_rate = compute_dynamic_fee_rate(
        cache.base_trade_fee_rate,
        quote.confidence_mantissa,
        quote.price_mantissa,
    );
    Ok((effective_bid, effective_ask, dynamic_fee_rate))
}

const FEE_RATE_UNITS_PER_BPS: u64 = 100;

fn compute_dynamic_fee_rate(
    base_trade_fee_rate: u64,
    confidence_mantissa: Option<i64>,
    price_mantissa: i64,
) -> u64 {
    if price_mantissa <= 0 {
        return base_trade_fee_rate.min(FEE_RATE_DENOMINATOR_VALUE.saturating_sub(1));
    }

    let confidence_bps = match confidence_mantissa {
        Some(c) if c > 0 => ((c as u128) * 10_000 / (price_mantissa as u128)) as u64,
        _ => 0,
    };

    base_trade_fee_rate
        .saturating_add(confidence_bps.saturating_mul(FEE_RATE_UNITS_PER_BPS))
        .min(FEE_RATE_DENOMINATOR_VALUE.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quote(price: i64) -> LatestQuote {
        LatestQuote {
            price_mantissa: price,
            confidence_mantissa: None,
            best_bid_mantissa: None,
            best_ask_mantissa: None,
            exponent: -8,
        }
    }

    #[tokio::test]
    async fn tick_returns_transient_when_quote_unavailable() {
        let cfg = test_config();
        let cache = PoolCache {
            min_spread_bps: 40,
            base_trade_fee_rate: 2_500,
        };
        let (_tx, rx) = watch::channel::<Option<LatestQuote>>(None);

        let result = tick(&cfg, &cache, &rx).await;
        match result {
            Err(TickError::Transient(msg)) => assert!(msg.contains("no Lazer quote")),
            other => panic!("expected Transient, got {other:?}"),
        }
    }

    #[test]
    fn compute_effective_uses_floor_when_no_oracle_book() {
        // Same expectation as `client::oracle_math` test
        // `spread_floor_only_no_oracle_bid_ask`: 40 bps total → ±20 each side.
        let q = quote(16_000_000_000);
        let cache = PoolCache {
            min_spread_bps: 40,
            base_trade_fee_rate: 2_500,
        };
        let (bid, ask, dynamic_fee_rate) = compute_update_fields(q, cache).unwrap();
        assert_eq!(bid, 15_968_000_000);
        assert_eq!(ask, 16_032_000_000);
        assert_eq!(dynamic_fee_rate, 2_500);
    }

    #[test]
    fn compute_effective_widens_with_confidence() {
        // 10 bps confidence on a $160 mid bumps the floor from 40 → 50 bps total.
        let mut q = quote(16_000_000_000);
        q.confidence_mantissa = Some(16_000_000); // 10 bps of price
        let cache = PoolCache {
            min_spread_bps: 40,
            base_trade_fee_rate: 2_500,
        };
        let (bid, ask, dynamic_fee_rate) = compute_update_fields(q, cache).unwrap();
        // 50 bps total = ±25 each side: 160 * 0.9975 = 159.60, 160 * 1.0025 = 160.40
        assert_eq!(bid, 15_960_000_000);
        assert_eq!(ask, 16_040_000_000);
        assert_eq!(dynamic_fee_rate, 3_500);
    }

    #[test]
    fn compute_effective_rejects_non_positive_price() {
        let q = quote(0);
        let cache = PoolCache {
            min_spread_bps: 40,
            base_trade_fee_rate: 2_500,
        };
        assert!(compute_update_fields(q, cache).is_err());
    }

    fn test_config() -> Config {
        use solana_sdk::pubkey::Pubkey;
        use solana_sdk::signature::Keypair;
        use std::sync::Arc;
        use std::time::Duration;
        Config {
            rpc_url: "http://localhost".to_string(),
            keypair: Arc::new(Keypair::new()),
            pool: Pubkey::new_unique(),
            feed_id: 1,
            channel: "fixed_rate@200ms".to_string(),
            lazer_token: "test".to_string(),
            interval: Duration::from_secs(1),
            program_id: Pubkey::new_unique(),
            compute_unit_price_micro_lamports: 0,
            compute_unit_limit: 0,
        }
    }
}
