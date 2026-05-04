# Nomad Finance Oracle AMM

Nomad is a Solana AMM that uses Pyth Lazer prices to quote like a prop DEX while remaining open to retail LPs. It is a fork of the [Raydium CPMM](https://github.com/raydium-io/raydium-cp-swap).

The on-chain program anchors quotes to a Pyth Lazer mid via a virtual-reserve construction. Spread widening (configured floor + age + confidence), Pyth signature verification, and dynamic fee selection happen **offchain** in a per-pool keeper bot. The bot pushes pre-computed `effective_bid_mantissa`, `effective_ask_mantissa`, and `dynamic_fee_rate` to PoolState via a permissioned `update_pool_oracle` instruction. Inventory skew is applied on-chain at swap time using the live reserves and per-pool params.

This split keeps swaps cheap (no Pyth CPI, no payload deserialization, no spread math on the hot path) while preserving the LP-protective economics.

## Deployments

### Mainnet-beta

| Item | Address |
|---|---|
| Program | `noMF5Do36aKFUUnDNMQRTMjc39DNczuEtESvUfZa84t` |
| Admin / upgrade authority | `3kXrf8w8Z6EjLJU4S8dAkpRL2von8z7Eh3kJnFrmo7Z2` |
| `create_pool_fee` receiver | `9WNaCaNpU85yUCq5LrvpLKyucA4jAT3up1KJD1P2G34C` (wSOL ATA owned by `3kXrf8w8…`) |

#### AmmConfig index 0 — `9zoEAzB9TyT6NTFTr1rqFXpWHavecijs1GsqRqSgiKwg`

Zero-fee config. All rates are set to `0` (denominator: `1_000_000`).

| Field | Value |
|---|---:|
| `trade_fee_rate` | 0 |
| `protocol_fee_rate` | 0 |
| `fund_fee_rate` | 0 |
| `create_pool_fee` | 0 lamports |
| `creator_fee_rate` | 0 |

## Offchain oracle keeper bot

Each pool stores an `oracle_keeper: Pubkey` set at init and rotatable by the global admin via `set_oracle_keeper`. Only that keeper can call `update_pool_oracle`. The on-chain swap path trusts the cached values without re-verifying the Pyth payload — freshness is the bot's responsibility.

### Single source of truth for the spread math

The widening math lives in `client/src/oracle_math.rs` as pure Rust with no external dependencies. To write the bot in Rust, take a path dependency on this repo's `client` crate:

```toml
# bot/Cargo.toml
[dependencies]
client = { path = "../raydium-cp-swap/client" }
```

```rust
use client::oracle_math::{
    compute_dynamic_min_spread_bps,
    compute_effective_bid_ask_mantissas,
};
```

### Inputs the bot fetches per cycle

From the Pyth Lazer websocket (or pull oracle), per the pool's configured `pyth_price_feed_id`:

- `price_mantissa: i64`, `exponent: i16`
- `best_bid_mantissa: Option<i64>`, `best_ask_mantissa: Option<i64>`
- `confidence_mantissa: Option<i64>`
- `publish_timestamp_us: u64`

From the pool's PoolState (read once at startup, refreshed on config change):

- `min_spread_bps`, `inventory_skew_*` — inventory params are consumed on-chain; the bot only needs `min_spread_bps`
- The pool's `amm_config.trade_fee_rate` as the fee floor

### Pipeline

```rust
use client::oracle_math::{
    compute_dynamic_min_spread_bps,
    compute_effective_bid_ask_mantissas,
};

// 1. Dynamic min spread (age + confidence widening)
let payload_age_us = (now_us as u64).saturating_sub(publish_timestamp_us);
let dynamic_min_spread_bps = compute_dynamic_min_spread_bps(
    pool.min_spread_bps,
    payload_age_us,
    confidence_mantissa,
    price_mantissa,
);

// 2. Effective bid/ask mantissas (Pyth bid/ask only WIDEN, never tighten)
let (effective_bid, effective_ask) = compute_effective_bid_ask_mantissas(
    price_mantissa,
    best_bid_mantissa,
    best_ask_mantissa,
    dynamic_min_spread_bps,
).expect("non-positive price or overflow");

// 3. Dynamic fee rate — bot's own policy, not in oracle_math.rs.
//    Suggested starting formula: widen the AMM-config base by a confidence-
//    proxied volatility term, capped at MAX_FEE_RATE.
let conf_ratio_bps: u64 = confidence_mantissa
    .filter(|&c| c > 0)
    .map(|c| (c as u128 * 10_000 / price_mantissa as u128) as u64)
    .unwrap_or(0);
let dynamic_fee_rate = (amm_config.trade_fee_rate
    + conf_ratio_bps * FEE_VOLATILITY_MULTIPLIER)
    .min(MAX_FEE_RATE);

// 4. Push to chain
program
    .request()
    .args(UpdatePoolOracle {
        effective_bid_mantissa: effective_bid,
        effective_ask_mantissa: effective_ask,
        price_exponent: exponent,
        dynamic_fee_rate,
    })
    .signer(oracle_keeper)
    .send()
    .await?;
```

### Cadence

Once per second matches Pyth Lazer's 1s free-tier cadence. Skip the push when values haven't moved by more than a configurable epsilon — saves lamports and reduces account-write contention.

### Constants and caps (re-exported from `oracle_math.rs`)

- `AGE_SPREAD_FREE_WINDOW_US = 1_000_000` — first 1s of age adds no extra spread
- `AGE_SPREAD_BPS_PER_SECOND = 1` — +1 bps per second after that
- `MAX_AGE_SPREAD_BPS = 9` — age widening capped at +9 bps
- `CONFIDENCE_SPREAD_MULTIPLIER = 1` — confidence ratio scaled 1:1
- `MAX_CONFIDENCE_SPREAD_BPS = 20` — confidence widening capped at +20 bps

Tune these in `client/src/oracle_math.rs`; the bot picks them up at recompile.

### Inventory skew (still on-chain)

The program applies inventory skew to the cached bid/ask before pricing the trade, using the pool's live reserves and the per-pool `inventory_skew_*` params (deadzone / bps_per_pct / max_bps). The bot does not compute or push skew.

### Liveness

If the bot stops pushing, swaps continue using the last cached values. The chain does not enforce a freshness deadline. Operators should monitor `last_oracle_update_unix` and alert when it lags expected cadence.

## On-chain swap economics (summary)

The chain-side flow per swap:

1. Read `effective_bid_mantissa`, `effective_ask_mantissa`, `price_exponent` from PoolState (`OracleNotInitialized` if the bot hasn't pushed yet).
2. Compute inventory imbalance from live reserves valued at the cached mid; derive a signed skew shift (gated by `inventory_skew_enabled`).
3. Pick bid (sell token_0) or ask (buy token_0) per direction; apply the skew shift; convert mantissa → raw token_1/token_0 fraction.
4. Run the constant-product virtual-reserve kernel using `pool_state.dynamic_fee_rate` (or `amm_config.trade_fee_rate` when the keeper hasn't pushed a non-zero override).
5. Transfer; emit `SwapEvent`.
