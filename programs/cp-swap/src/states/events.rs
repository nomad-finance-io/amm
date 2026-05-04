use anchor_lang::prelude::*;

/// Emitted when deposit and withdraw
#[event]
#[cfg_attr(feature = "client", derive(Debug))]
pub struct LpChangeEvent {
    pub pool_id: Pubkey,
    pub lp_amount_before: u64,
    /// pool vault sub trade fees
    pub token_0_vault_before: u64,
    /// pool vault sub trade fees
    pub token_1_vault_before: u64,
    /// calculate result without transfer fee
    pub token_0_amount: u64,
    /// calculate result without transfer fee
    pub token_1_amount: u64,
    pub token_0_transfer_fee: u64,
    pub token_1_transfer_fee: u64,
    // 0: deposit, 1: withdraw
    pub change_type: u8,
}

/// Emitted when swap
#[event]
#[cfg_attr(feature = "client", derive(Debug))]
pub struct SwapEvent {
    pub pool_id: Pubkey,
    /// pool vault sub trade fees
    pub input_vault_before: u64,
    /// pool vault sub trade fees
    pub output_vault_before: u64,
    /// calculate result without transfer fee
    pub input_amount: u64,
    /// calculate result without transfer fee
    pub output_amount: u64,
    pub input_transfer_fee: u64,
    pub output_transfer_fee: u64,
    pub base_input: bool,
    pub input_mint: Pubkey,
    pub output_mint: Pubkey,
    pub trade_fee: u64,
    /// Amount of fee tokens going to creator
    pub creator_fee: u64,
    pub creator_fee_on_input: bool,
    /// Cached effective bid mantissa applied to this swap (canonical token_1/token_0
    /// direction). Set offchain by the per-pool keeper bot.
    pub oracle_bid: i64,
    /// Cached effective ask mantissa applied to this swap.
    pub oracle_ask: i64,
    /// Decimal exponent for the bid/ask mantissas above.
    pub oracle_exponent: i16,
    /// Unix timestamp (seconds) at which the keeper bot last pushed oracle values.
    /// Telemetry only — used by indexers to measure cache freshness.
    pub oracle_last_update_unix: i64,
    /// Trade-fee rate actually applied to this swap, in fee-rate units
    /// (denominator 1_000_000, same as `AmmConfig.trade_fee_rate`). When the
    /// keeper has set `dynamic_fee_rate > 0` this is that value; otherwise
    /// the AMM-config base rate.
    pub effective_trade_fee_rate: u64,
    /// Value-weighted inventory imbalance in bps at the moment of the swap:
    /// `(value_0 - value_1) / (value_0 + value_1) × 10_000`.
    /// Positive = token_0 surplus; 0 = balanced; negative = token_1 surplus.
    pub inventory_imbalance_bps: i32,
    /// Signed skew shift (bps) applied to the canonical mantissa before
    /// picking bid/ask. Positive raises canonical, negative lowers it.
    /// 0 when disabled, inside the dead zone, or the imbalance could not be computed.
    pub inventory_skew_bps: i32,
}

/// Emitted when the per-pool oracle keeper bot pushes a fresh bid/ask + fee.
#[event]
#[cfg_attr(feature = "client", derive(Debug))]
pub struct OracleUpdated {
    pub pool_id: Pubkey,
    pub effective_bid_mantissa: i64,
    pub effective_ask_mantissa: i64,
    pub price_exponent: i16,
    pub dynamic_fee_rate: u64,
    pub slot: u64,
    pub unix_timestamp: i64,
}
