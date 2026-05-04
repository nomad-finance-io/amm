pub mod curve;
pub mod error;
pub mod instructions;
pub mod states;
pub mod utils;
use crate::curve::fees::FEE_RATE_DENOMINATOR_VALUE;
use anchor_lang::prelude::*;
use instructions::*;
pub use states::CreatorFeeOn;

#[cfg(not(feature = "no-entrypoint"))]
solana_security_txt::security_txt! {
    name: "nomad-amm",
    project_url: "https://raydium.io",
    contacts: "link:https://immunefi.com/bounty/raydium",
    policy: "https://immunefi.com/bounty/raydium",
    source_code: "https://github.com/raydium-io/raydium-cp-swap",
    preferred_languages: "en",
    auditors: "https://github.com/raydium-io/raydium-docs/blob/master/audit/MadShield%20Q1%202024/raydium-cp-swap-v-1.0.0.pdf"
}

declare_id!("noMF5Do36aKFUUnDNMQRTMjc39DNczuEtESvUfZa84t");

pub mod admin {
    use super::{pubkey, Pubkey};
    pub const ID: Pubkey = pubkey!("3kXrf8w8Z6EjLJU4S8dAkpRL2von8z7Eh3kJnFrmo7Z2");
}

pub mod create_pool_fee_reveiver {
    use super::{pubkey, Pubkey};
    // wSOL ATA owned by 3kXrf8w8Z6EjLJU4S8dAkpRL2von8z7Eh3kJnFrmo7Z2.
    pub const ID: Pubkey = pubkey!("9WNaCaNpU85yUCq5LrvpLKyucA4jAT3up1KJD1P2G34C");
}

pub const AUTH_SEED: &str = "vault_and_lp_mint_auth_seed";

#[program]
pub mod nomad_amm {
    use super::*;

    // The configuration of AMM protocol, include trade fee and protocol fee
    /// # Arguments
    ///
    /// * `ctx`- The accounts needed by instruction.
    /// * `index` - The index of amm config, there may be multiple config.
    /// * `trade_fee_rate` - Trade fee rate, can be changed.
    /// * `protocol_fee_rate` - The rate of protocol fee within trade fee.
    /// * `fund_fee_rate` - The rate of fund fee within trade fee.
    ///
    pub fn create_amm_config(
        ctx: Context<CreateAmmConfig>,
        index: u16,
        trade_fee_rate: u64,
        protocol_fee_rate: u64,
        fund_fee_rate: u64,
        create_pool_fee: u64,
        creator_fee_rate: u64,
    ) -> Result<()> {
        assert!(trade_fee_rate + creator_fee_rate < FEE_RATE_DENOMINATOR_VALUE);
        assert!(protocol_fee_rate <= FEE_RATE_DENOMINATOR_VALUE);
        assert!(fund_fee_rate <= FEE_RATE_DENOMINATOR_VALUE);
        assert!(fund_fee_rate + protocol_fee_rate <= FEE_RATE_DENOMINATOR_VALUE);
        instructions::create_amm_config(
            ctx,
            index,
            trade_fee_rate,
            protocol_fee_rate,
            fund_fee_rate,
            create_pool_fee,
            creator_fee_rate,
        )
    }

    /// Updates the owner of the amm config
    /// Must be called by the current owner or admin
    ///
    /// # Arguments
    ///
    /// * `ctx`- The context of accounts
    /// * `trade_fee_rate`- The new trade fee rate of amm config, be set when `param` is 0
    /// * `protocol_fee_rate`- The new protocol fee rate of amm config, be set when `param` is 1
    /// * `fund_fee_rate`- The new fund fee rate of amm config, be set when `param` is 2
    /// * `new_owner`- The config's new owner, be set when `param` is 3
    /// * `new_fund_owner`- The config's new fund owner, be set when `param` is 4
    /// * `param`- The value can be 0 | 1 | 2 | 3 | 4, otherwise will report a error
    ///
    pub fn update_amm_config(ctx: Context<UpdateAmmConfig>, param: u8, value: u64) -> Result<()> {
        instructions::update_amm_config(ctx, param, value)
    }

    /// Update pool status for given value
    ///
    /// # Arguments
    ///
    /// * `ctx`- The context of accounts
    /// * `status` - The value of status
    ///
    pub fn update_pool_status(ctx: Context<UpdatePoolStatus>, status: u8) -> Result<()> {
        instructions::update_pool_status(ctx, status)
    }

    /// Collect the protocol fee accrued to the pool
    ///
    /// # Arguments
    ///
    /// * `ctx` - The context of accounts
    /// * `amount_0_requested` - The maximum amount of token_0 to send, can be 0 to collect fees in only token_1
    /// * `amount_1_requested` - The maximum amount of token_1 to send, can be 0 to collect fees in only token_0
    ///
    pub fn collect_protocol_fee(
        ctx: Context<CollectProtocolFee>,
        amount_0_requested: u64,
        amount_1_requested: u64,
    ) -> Result<()> {
        instructions::collect_protocol_fee(ctx, amount_0_requested, amount_1_requested)
    }

    /// Collect the fund fee accrued to the pool
    ///
    /// # Arguments
    ///
    /// * `ctx` - The context of accounts
    /// * `amount_0_requested` - The maximum amount of token_0 to send, can be 0 to collect fees in only token_1
    /// * `amount_1_requested` - The maximum amount of token_1 to send, can be 0 to collect fees in only token_0
    ///
    pub fn collect_fund_fee(
        ctx: Context<CollectFundFee>,
        amount_0_requested: u64,
        amount_1_requested: u64,
    ) -> Result<()> {
        instructions::collect_fund_fee(ctx, amount_0_requested, amount_1_requested)
    }

    /// Collect the creator fee
    ///
    /// # Arguments
    ///
    /// * `ctx` - The context of accounts
    ///
    pub fn collect_creator_fee(ctx: Context<CollectCreatorFee>) -> Result<()> {
        instructions::collect_creator_fee(ctx)
    }

    /// Create a permission account
    ///
    /// # Arguments
    ///
    /// * `ctx`- The context of accounts
    ///
    pub fn create_permission_pda(ctx: Context<CreatePermissionPda>) -> Result<()> {
        instructions::create_permission_pda(ctx)
    }

    /// Close a permission account
    ///
    /// # Arguments
    ///
    /// * `ctx`- The context of accounts
    ///
    pub fn close_permission_pda(ctx: Context<ClosePermissionPda>) -> Result<()> {
        instructions::close_permission_pda(ctx)
    }

    /// Push pre-computed oracle bid/ask + dynamic fee to a pool's cache.
    /// Signed by the pool's per-pool oracle keeper. The widening math runs
    /// offchain in `client/src/oracle_math.rs`; this instruction is a thin
    /// validated write.
    ///
    /// # Arguments
    /// * `effective_bid_mantissa` - Bot-computed bid (canonical token_1/token_0 direction)
    /// * `effective_ask_mantissa` - Bot-computed ask, must be >= bid
    /// * `price_exponent` - Pyth exponent for the bid/ask mantissas
    /// * `dynamic_fee_rate` - Fee rate (FEE_RATE_DENOMINATOR_VALUE-scaled). 0 falls back to amm_config.trade_fee_rate.
    pub fn update_pool_oracle(
        ctx: Context<UpdatePoolOracle>,
        effective_bid_mantissa: i64,
        effective_ask_mantissa: i64,
        price_exponent: i16,
        dynamic_fee_rate: u64,
    ) -> Result<()> {
        instructions::update_pool_oracle(
            ctx,
            effective_bid_mantissa,
            effective_ask_mantissa,
            price_exponent,
            dynamic_fee_rate,
        )
    }

    /// Rotate a pool's oracle keeper. Global admin only.
    pub fn set_oracle_keeper(ctx: Context<SetOracleKeeper>, new_keeper: Pubkey) -> Result<()> {
        instructions::set_oracle_keeper(ctx, new_keeper)
    }

    /// Creates a pool for the given token pair and the initial price
    ///
    /// # Arguments
    ///
    /// * `ctx`- The context of accounts
    /// * `init_amount_0` - the initial amount_0 to deposit
    /// * `init_amount_1` - the initial amount_1 to deposit
    /// * `open_time` - the timestamp allowed for swap
    /// * `pyth_price_feed_id` - Pyth Lazer feed id this pool will be priced against (must be non-zero)
    /// * `min_spread_bps` - Minimum total spread floor in basis points (1..=1000)
    /// * `inventory_skew_enabled` - Whether to apply inventory-based quote skew
    /// * `inventory_skew_deadzone_bps` - Imbalance (bps of value) inside which no skew is applied (<=5000)
    /// * `inventory_skew_bps_per_pct` - Bps of skew per 1% imbalance beyond dead zone (<=100)
    /// * `inventory_skew_max_bps` - Hard cap on |skew| in bps (<=1000)
    /// * `oracle_keeper` - Pubkey of the per-pool keeper bot allowed to push oracle updates
    ///
    pub fn initialize(
        ctx: Context<Initialize>,
        init_amount_0: u64,
        init_amount_1: u64,
        open_time: u64,
        pyth_price_feed_id: u32,
        min_spread_bps: u16,
        inventory_skew_enabled: bool,
        inventory_skew_deadzone_bps: u16,
        inventory_skew_bps_per_pct: u16,
        inventory_skew_max_bps: u16,
        oracle_keeper: Pubkey,
    ) -> Result<()> {
        instructions::initialize(
            ctx,
            init_amount_0,
            init_amount_1,
            open_time,
            pyth_price_feed_id,
            min_spread_bps,
            inventory_skew_enabled,
            inventory_skew_deadzone_bps,
            inventory_skew_bps_per_pct,
            inventory_skew_max_bps,
            oracle_keeper,
        )
    }

    /// Create a pool with permission
    ///
    /// # Arguments
    ///
    /// * `ctx`- The context of accounts
    /// * `init_amount_0` - the initial amount_0 to deposit
    /// * `init_amount_1` - the initial amount_1 to deposit
    /// * `open_time` - the timestamp allowed for swap
    /// * `creator_fee_on` - creator fee model, 0：both token0 and token1 (depends on the input), 1: only token0, 2: only token1
    /// * `pyth_price_feed_id` - Pyth Lazer feed id this pool will be priced against (must be non-zero)
    /// * `min_spread_bps` - Minimum total spread floor in basis points (1..=1000)
    /// * `inventory_skew_enabled` - Whether to apply inventory-based quote skew
    /// * `inventory_skew_deadzone_bps` - Imbalance (bps of value) inside which no skew is applied (<=5000)
    /// * `inventory_skew_bps_per_pct` - Bps of skew per 1% imbalance beyond dead zone (<=100)
    /// * `inventory_skew_max_bps` - Hard cap on |skew| in bps (<=1000)
    ///
    pub fn initialize_with_permission(
        ctx: Context<InitializeWithPermission>,
        init_amount_0: u64,
        init_amount_1: u64,
        open_time: u64,
        creator_fee_on: CreatorFeeOn,
        pyth_price_feed_id: u32,
        min_spread_bps: u16,
        inventory_skew_enabled: bool,
        inventory_skew_deadzone_bps: u16,
        inventory_skew_bps_per_pct: u16,
        inventory_skew_max_bps: u16,
        oracle_keeper: Pubkey,
    ) -> Result<()> {
        instructions::initialize_with_permission(
            ctx,
            init_amount_0,
            init_amount_1,
            open_time,
            creator_fee_on,
            pyth_price_feed_id,
            min_spread_bps,
            inventory_skew_enabled,
            inventory_skew_deadzone_bps,
            inventory_skew_bps_per_pct,
            inventory_skew_max_bps,
            oracle_keeper,
        )
    }

    /// Deposit lp token to the pool
    ///
    /// # Arguments
    ///
    /// * `ctx`- The context of accounts
    /// * `lp_token_amount` - Increased number of LPs
    /// * `maximum_token_0_amount` -  Maximum token 0 amount to deposit, prevents excessive slippage
    /// * `maximum_token_1_amount` - Maximum token 1 amount to deposit, prevents excessive slippage
    ///
    pub fn deposit(
        ctx: Context<Deposit>,
        lp_token_amount: u64,
        maximum_token_0_amount: u64,
        maximum_token_1_amount: u64,
    ) -> Result<()> {
        instructions::deposit(
            ctx,
            lp_token_amount,
            maximum_token_0_amount,
            maximum_token_1_amount,
        )
    }

    /// Withdraw lp for token0 and token1
    ///
    /// # Arguments
    ///
    /// * `ctx`- The context of accounts
    /// * `lp_token_amount` - Amount of pool tokens to burn. User receives an output of token a and b based on the percentage of the pool tokens that are returned.
    /// * `minimum_token_0_amount` -  Minimum amount of token 0 to receive, prevents excessive slippage
    /// * `minimum_token_1_amount` -  Minimum amount of token 1 to receive, prevents excessive slippage
    ///
    pub fn withdraw(
        ctx: Context<Withdraw>,
        lp_token_amount: u64,
        minimum_token_0_amount: u64,
        minimum_token_1_amount: u64,
    ) -> Result<()> {
        instructions::withdraw(
            ctx,
            lp_token_amount,
            minimum_token_0_amount,
            minimum_token_1_amount,
        )
    }

    /// Swap the tokens in the pool base input amount.
    ///
    /// Reads the cached oracle bid/ask + dynamic fee from PoolState (pushed by
    /// the per-pool keeper bot via `update_pool_oracle`). No Pyth Lazer
    /// payload is verified on-chain.
    ///
    /// # Arguments
    ///
    /// * `ctx`- The context of accounts
    /// * `amount_in` -  input amount to transfer, output to DESTINATION is based on the exchange rate
    /// * `minimum_amount_out` -  Minimum amount of output token, prevents excessive slippage
    ///
    pub fn swap_base_input(
        ctx: Context<Swap>,
        amount_in: u64,
        minimum_amount_out: u64,
    ) -> Result<()> {
        instructions::swap_base_input(ctx, amount_in, minimum_amount_out)
    }

    /// Swap the tokens in the pool base output amount.
    ///
    /// Reads the cached oracle bid/ask + dynamic fee from PoolState (pushed by
    /// the per-pool keeper bot via `update_pool_oracle`). No Pyth Lazer
    /// payload is verified on-chain.
    ///
    /// # Arguments
    ///
    /// * `ctx`- The context of accounts
    /// * `max_amount_in` -  input amount prevents excessive slippage
    /// * `amount_out` -  amount of output token
    ///
    pub fn swap_base_output(
        ctx: Context<Swap>,
        max_amount_in: u64,
        amount_out: u64,
    ) -> Result<()> {
        instructions::swap_base_output(ctx, max_amount_in, amount_out)
    }
}
