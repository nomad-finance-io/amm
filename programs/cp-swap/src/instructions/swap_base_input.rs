use crate::curve::calculator::{CurveCalculator, TradeDirection};
use crate::curve::oracle_curve::{
    compute_inventory_imbalance_bps, compute_inventory_skew_bps, pyth_price_to_raw_fraction,
    shift_mantissa_by_signed_bps,
};
use crate::error::ErrorCode;
use crate::states::*;
use crate::utils::token::*;
use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

#[derive(Accounts)]
pub struct Swap<'info> {
    /// The user performing the swap
    #[account(mut)]
    pub payer: Signer<'info>,

    /// CHECK: pool vault and lp mint authority
    #[account(
        seeds = [
            crate::AUTH_SEED.as_bytes(),
        ],
        bump,
    )]
    pub authority: UncheckedAccount<'info>,

    /// The factory state to read protocol fees
    #[account(address = pool_state.load()?.amm_config)]
    pub amm_config: Box<Account<'info, AmmConfig>>,

    /// The program account of the pool in which the swap will be performed
    #[account(mut)]
    pub pool_state: AccountLoader<'info, PoolState>,

    /// The user token account for input token
    #[account(mut)]
    pub input_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The user token account for output token
    #[account(mut)]
    pub output_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The vault token account for input token
    #[account(
        mut,
        constraint = input_vault.key() == pool_state.load()?.token_0_vault || input_vault.key() == pool_state.load()?.token_1_vault
    )]
    pub input_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The vault token account for output token
    #[account(
        mut,
        constraint = output_vault.key() == pool_state.load()?.token_0_vault || output_vault.key() == pool_state.load()?.token_1_vault
    )]
    pub output_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    /// SPL program for input token transfers
    pub input_token_program: Interface<'info, TokenInterface>,

    /// SPL program for output token transfers
    pub output_token_program: Interface<'info, TokenInterface>,

    /// The mint of input token
    #[account(
        address = input_vault.mint
    )]
    pub input_token_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The mint of output token
    #[account(
        address = output_vault.mint
    )]
    pub output_token_mint: Box<InterfaceAccount<'info, Mint>>,
    /// The program account for the most recent oracle observation
    #[account(mut, address = pool_state.load()?.observation_key)]
    pub observation_state: AccountLoader<'info, ObservationState>,
}

pub fn swap_base_input(
    ctx: Context<Swap>,
    amount_in: u64,
    minimum_amount_out: u64,
) -> Result<()> {
    // One Clock sysvar fetch — reused for the pool open-time check, the
    // observation update, and `recent_epoch` below.
    let clock = Clock::get()?;
    let block_timestamp = clock.unix_timestamp.max(0) as u64;
    let pool_id = ctx.accounts.pool_state.key();

    // Read cached oracle bid/ask + exponent pushed by the keeper bot. The
    // bid/ask widening + Pyth verification happens offchain in
    // `client/src/oracle_math.rs` — the chain trusts the pushed values and
    // does not re-verify the source payload.
    let pool_state = &mut ctx.accounts.pool_state.load_mut()?;
    let (cached_bid, cached_ask, cached_exponent) = pool_state
        .cached_oracle_quote()
        .ok_or(ErrorCode::OracleNotInitialized)?;
    let last_oracle_update_unix = pool_state.last_oracle_update_unix;

    if !pool_state.get_status_by_bit(PoolStatusBitIndex::Swap)
        || block_timestamp < pool_state.open_time
    {
        return err!(ErrorCode::NotApproved);
    }

    let transfer_fee =
        get_transfer_fee(&ctx.accounts.input_token_mint.to_account_info(), amount_in)?;
    // Take transfer fees into account for actual amount transferred in
    let actual_amount_in = amount_in.saturating_sub(transfer_fee);
    require_gt!(actual_amount_in, 0);

    let SwapParams {
        trade_direction,
        total_input_token_amount,
        total_output_token_amount,
        token_0_price_x64,
        token_1_price_x64,
        is_creator_fee_on_input,
    } = pool_state.get_swap_params(
        ctx.accounts.input_vault.key(),
        ctx.accounts.output_vault.key(),
        ctx.accounts.input_vault.amount,
        ctx.accounts.output_vault.amount,
    )?;

    // Inventory skew: shift the cached bid/ask by the same signed amount toward
    // rebalancing if value-weighted pool inventory has drifted from 50/50.
    // Applied AFTER the offchain bid/ask widening, capped by max_bps so the
    // round-trip cost still bounds the rebalancing incentive.
    let (inventory_imbalance_bps, inventory_skew_bps, bid_mantissa, ask_mantissa) =
        if pool_state.inventory_skew_enabled != 0 {
            // Mid-price for the imbalance valuation: average of cached bid/ask.
            let mid_mantissa =
                ((cached_bid as i128 + cached_ask as i128) / 2) as i64;
            let (mid_num, mid_den) = pyth_price_to_raw_fraction(
                mid_mantissa,
                cached_exponent,
                pool_state.mint_0_decimals,
                pool_state.mint_1_decimals,
            )
            .ok_or(ErrorCode::InvalidEffectiveSpread)?;
            // Re-label net amounts back to canonical (token_0, token_1) order.
            let (token_0_net, token_1_net) = match trade_direction {
                TradeDirection::ZeroForOne => (total_input_token_amount, total_output_token_amount),
                TradeDirection::OneForZero => (total_output_token_amount, total_input_token_amount),
            };
            let imbalance =
                compute_inventory_imbalance_bps(token_0_net, token_1_net, mid_num, mid_den)
                    .unwrap_or(0);
            let skew = compute_inventory_skew_bps(
                imbalance,
                pool_state.inventory_skew_deadzone_bps,
                pool_state.inventory_skew_bps_per_pct,
                pool_state.inventory_skew_max_bps,
            );
            let shifted_bid = shift_mantissa_by_signed_bps(cached_bid, skew)
                .ok_or(ErrorCode::InvalidInventorySkewShift)?;
            let shifted_ask = shift_mantissa_by_signed_bps(cached_ask, skew)
                .ok_or(ErrorCode::InvalidInventorySkewShift)?;
            (imbalance, skew, shifted_bid, shifted_ask)
        } else {
            (0i32, 0i32, cached_bid, cached_ask)
        };

    // ZeroForOne: user sells token_0 to pool → pool applies BID (lower price).
    // OneForZero: user buys token_0 from pool → pool applies ASK (higher price).
    let effective_mantissa = match trade_direction {
        TradeDirection::ZeroForOne => bid_mantissa,
        TradeDirection::OneForZero => ask_mantissa,
    };
    // Canonical raw token_1 / token_0 fraction from the spread-adjusted mantissa.
    let (canonical_num, canonical_den) = pyth_price_to_raw_fraction(
        effective_mantissa,
        cached_exponent,
        pool_state.mint_0_decimals,
        pool_state.mint_1_decimals,
    )
    .ok_or(ErrorCode::InvalidEffectiveSpread)?;
    // Orient as output-per-input for the current direction.
    let (spot_num, spot_den) = match trade_direction {
        TradeDirection::ZeroForOne => (canonical_num, canonical_den), // output=token_1
        TradeDirection::OneForZero => (canonical_den, canonical_num), // output=token_0
    };

    let effective_trade_fee_rate =
        pool_state.effective_trade_fee_rate(ctx.accounts.amm_config.trade_fee_rate);

    let creator_fee_rate =
        pool_state.adjust_creator_fee_rate(ctx.accounts.amm_config.creator_fee_rate);
    let result = CurveCalculator::swap_base_input(
        u128::from(actual_amount_in),
        u128::from(total_input_token_amount),
        u128::from(total_output_token_amount),
        effective_trade_fee_rate,
        creator_fee_rate,
        ctx.accounts.amm_config.protocol_fee_rate,
        ctx.accounts.amm_config.fund_fee_rate,
        is_creator_fee_on_input,
        spot_num,
        spot_den,
    )
    .ok_or(ErrorCode::ZeroTradingTokens)?;

    #[cfg(feature = "enable-log")]
    msg!(
        "input_amount:{}, output_amount:{}, trade_fee:{}, input_transfer_fee:{}, is_creator_fee_on_input:{}, creator_fee:{}, cached_bid:{}, cached_ask:{}, cached_exp:{}",
        result.input_amount,
        result.output_amount,
        result.trade_fee,
        transfer_fee,
        is_creator_fee_on_input,
        result.creator_fee,
        cached_bid,
        cached_ask,
        cached_exponent,
    );
    require_eq!(
        u64::try_from(result.input_amount).unwrap(),
        actual_amount_in
    );
    let (input_transfer_amount, input_transfer_fee) = (amount_in, transfer_fee);
    let (output_transfer_amount, output_transfer_fee) = {
        let amount_out = u64::try_from(result.output_amount).unwrap();
        let transfer_fee = get_transfer_fee(
            &ctx.accounts.output_token_mint.to_account_info(),
            amount_out,
        )?;
        let amount_received = amount_out.checked_sub(transfer_fee).unwrap();
        require_gt!(amount_received, 0);
        require_gte!(
            amount_received,
            minimum_amount_out,
            ErrorCode::ExceededSlippage
        );
        (amount_out, transfer_fee)
    };

    pool_state.update_fees(
        u64::try_from(result.protocol_fee).unwrap(),
        u64::try_from(result.fund_fee).unwrap(),
        u64::try_from(result.creator_fee).unwrap(),
        trade_direction,
    )?;

    emit!(SwapEvent {
        pool_id,
        input_vault_before: total_input_token_amount,
        output_vault_before: total_output_token_amount,
        input_amount: u64::try_from(result.input_amount).unwrap(),
        output_amount: u64::try_from(result.output_amount).unwrap(),
        input_transfer_fee,
        output_transfer_fee,
        base_input: true,
        input_mint: ctx.accounts.input_token_mint.key(),
        output_mint: ctx.accounts.output_token_mint.key(),
        trade_fee: u64::try_from(result.trade_fee).unwrap(),
        creator_fee: u64::try_from(result.creator_fee).unwrap(),
        creator_fee_on_input: is_creator_fee_on_input,
        oracle_bid: bid_mantissa,
        oracle_ask: ask_mantissa,
        oracle_exponent: cached_exponent,
        oracle_last_update_unix: last_oracle_update_unix,
        effective_trade_fee_rate,
        inventory_imbalance_bps,
        inventory_skew_bps,
    });
    // The classic `new_k >= old_k` invariant no longer holds when pricing is
    // oracle-anchored: actual reserves can drift across swaps as the pool
    // rebalances around the oracle ratio. The invariant now lives implicitly
    // in the virtual-reserve kernel (which preserves `x_v * y_v = k`).

    transfer_from_user_to_pool_vault(
        ctx.accounts.payer.to_account_info(),
        ctx.accounts.input_token_account.to_account_info(),
        ctx.accounts.input_vault.to_account_info(),
        ctx.accounts.input_token_mint.to_account_info(),
        ctx.accounts.input_token_program.to_account_info(),
        input_transfer_amount,
        ctx.accounts.input_token_mint.decimals,
    )?;

    transfer_from_pool_vault_to_user(
        ctx.accounts.authority.to_account_info(),
        ctx.accounts.output_vault.to_account_info(),
        ctx.accounts.output_token_account.to_account_info(),
        ctx.accounts.output_token_mint.to_account_info(),
        ctx.accounts.output_token_program.to_account_info(),
        output_transfer_amount,
        ctx.accounts.output_token_mint.decimals,
        &[&[crate::AUTH_SEED.as_bytes(), &[pool_state.auth_bump]]],
    )?;

    // update the previous price to the observation
    ctx.accounts.observation_state.load_mut()?.update(
        block_timestamp,
        token_0_price_x64,
        token_1_price_x64,
    )?;
    pool_state.recent_epoch = clock.epoch;

    Ok(())
}
