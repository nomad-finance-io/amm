use crate::curve::fees::FEE_RATE_DENOMINATOR_VALUE;
use crate::error::ErrorCode;
use crate::states::*;
use anchor_lang::prelude::*;

/// Per-pool keeper-bot push of pre-computed oracle values.
///
/// The keeper bot does the Pyth Lazer signature verification and the bid/ask
/// widening offchain (see `client/src/oracle_math.rs`) and pushes the
/// pre-computed values + dynamic fee rate here. The on-chain swap path reads
/// these cached values; it does not re-verify the source payload.
#[derive(Accounts)]
pub struct UpdatePoolOracle<'info> {
    /// The pool's oracle keeper. Set at pool init, rotatable by the global
    /// admin via `set_oracle_keeper`.
    pub oracle_keeper: Signer<'info>,

    #[account(mut, has_one = oracle_keeper @ ErrorCode::InvalidOracleKeeper)]
    pub pool_state: AccountLoader<'info, PoolState>,
}

pub fn update_pool_oracle(
    ctx: Context<UpdatePoolOracle>,
    effective_bid_mantissa: i64,
    effective_ask_mantissa: i64,
    price_exponent: i16,
    dynamic_fee_rate: u64,
) -> Result<()> {
    require!(
        effective_bid_mantissa > 0,
        ErrorCode::InvalidEffectiveSpread
    );
    require!(
        effective_ask_mantissa >= effective_bid_mantissa,
        ErrorCode::BidNotLessThanAsk
    );
    require!(
        dynamic_fee_rate < FEE_RATE_DENOMINATOR_VALUE,
        ErrorCode::InvalidDynamicFeeRate
    );

    let mut pool_state = ctx.accounts.pool_state.load_mut()?;
    let clock = Clock::get()?;

    pool_state.effective_bid_mantissa = effective_bid_mantissa;
    pool_state.effective_ask_mantissa = effective_ask_mantissa;
    pool_state.price_exponent = price_exponent;
    pool_state.dynamic_fee_rate = dynamic_fee_rate;
    pool_state.last_oracle_update_slot = clock.slot;
    pool_state.last_oracle_update_unix = clock.unix_timestamp;

    emit!(OracleUpdated {
        pool_id: ctx.accounts.pool_state.key(),
        effective_bid_mantissa,
        effective_ask_mantissa,
        price_exponent,
        dynamic_fee_rate,
        slot: clock.slot,
        unix_timestamp: clock.unix_timestamp,
    });

    Ok(())
}

/// Rotate a pool's oracle keeper. Global admin only.
#[derive(Accounts)]
pub struct SetOracleKeeper<'info> {
    #[account(address = crate::admin::ID)]
    pub authority: Signer<'info>,

    #[account(mut)]
    pub pool_state: AccountLoader<'info, PoolState>,
}

pub fn set_oracle_keeper(ctx: Context<SetOracleKeeper>, new_keeper: Pubkey) -> Result<()> {
    let mut pool_state = ctx.accounts.pool_state.load_mut()?;
    pool_state.oracle_keeper = new_keeper;
    Ok(())
}
