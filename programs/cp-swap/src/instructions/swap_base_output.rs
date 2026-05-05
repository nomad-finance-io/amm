use super::swap_base_input::Swap;
use crate::curve::calculator::{CurveCalculator, TradeDirection};
use crate::curve::oracle_curve::{
    compute_inventory_imbalance_bps, compute_inventory_skew_bps, pyth_price_to_raw_fraction,
    shift_mantissa_by_signed_bps,
};
use crate::error::ErrorCode;
use crate::states::*;
use crate::utils::token::*;
use anchor_lang::prelude::*;

pub fn swap_base_output(
    ctx: Context<Swap>,
    max_amount_in: u64,
    amount_out_received: u64,
) -> Result<()> {
    require_gt!(amount_out_received, 0);
    // One Clock sysvar fetch — reused for open-time check, observation update,
    // and recent_epoch below. See swap_base_input for design rationale.
    let clock = Clock::get()?;
    let block_timestamp = clock.unix_timestamp.max(0) as u64;
    let pool_id = ctx.accounts.pool_state.key();

    // Read cached oracle bid/ask + exponent pushed by the keeper bot. See
    // swap_base_input for the full design rationale.
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
    let out_transfer_fee = get_transfer_inverse_fee(
        &ctx.accounts.output_token_mint.to_account_info(),
        amount_out_received,
    )?;
    let amount_out_with_transfer_fee = amount_out_received.checked_add(out_transfer_fee).unwrap();

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

    // Inventory skew: shift the cached quote toward rebalancing. See
    // swap_base_input for the full reasoning; logic here is identical.
    let (inventory_imbalance_bps, inventory_skew_bps, bid_mantissa, ask_mantissa) =
        if pool_state.inventory_skew_enabled != 0 {
            let mid_mantissa = ((cached_bid as i128 + cached_ask as i128) / 2) as i64;
            let (mid_num, mid_den) = pyth_price_to_raw_fraction(
                mid_mantissa,
                cached_exponent,
                pool_state.mint_0_decimals,
                pool_state.mint_1_decimals,
            )
            .ok_or(ErrorCode::InvalidEffectiveSpread)?;
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

    let effective_mantissa = match trade_direction {
        TradeDirection::ZeroForOne => bid_mantissa,
        TradeDirection::OneForZero => ask_mantissa,
    };
    let (canonical_num, canonical_den) = pyth_price_to_raw_fraction(
        effective_mantissa,
        cached_exponent,
        pool_state.mint_0_decimals,
        pool_state.mint_1_decimals,
    )
    .ok_or(ErrorCode::InvalidEffectiveSpread)?;
    let (spot_num, spot_den) = match trade_direction {
        TradeDirection::ZeroForOne => (canonical_num, canonical_den),
        TradeDirection::OneForZero => (canonical_den, canonical_num),
    };

    let effective_trade_fee_rate =
        pool_state.effective_trade_fee_rate(ctx.accounts.amm_config.trade_fee_rate);

    let creator_fee_rate =
        pool_state.adjust_creator_fee_rate(ctx.accounts.amm_config.creator_fee_rate);
    let result = CurveCalculator::swap_base_output(
        u128::from(amount_out_with_transfer_fee),
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
        "input_amount:{}, output_amount:{}, trade_fee:{}, output_transfer_fee:{}, is_creator_fee_on_input:{}, creator_fee:{}, cached_bid:{}, cached_ask:{}, cached_exp:{}",
        result.input_amount,
        result.output_amount,
        result.trade_fee,
        out_transfer_fee,
        is_creator_fee_on_input,
        result.creator_fee,
        cached_bid,
        cached_ask,
        cached_exponent,
    );

    // Re-calculate the source amount swapped based on what the curve says
    let (input_transfer_amount, input_transfer_fee) = {
        let input_amount = u64::try_from(result.input_amount).unwrap();
        require_gt!(input_amount, 0);
        let transfer_fee = get_transfer_inverse_fee(
            &ctx.accounts.input_token_mint.to_account_info(),
            input_amount,
        )?;
        let input_transfer_amount = input_amount.checked_add(transfer_fee).unwrap();
        require_gte!(
            max_amount_in,
            input_transfer_amount,
            ErrorCode::ExceededSlippage
        );
        (input_transfer_amount, transfer_fee)
    };
    require_eq!(
        u64::try_from(result.output_amount).unwrap(),
        amount_out_with_transfer_fee
    );
    let (output_transfer_amount, output_transfer_fee) =
        (amount_out_with_transfer_fee, out_transfer_fee);

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
        base_input: false,
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
    // Classic `new_k >= old_k` invariant does not hold under oracle anchoring;
    // the virtual-reserve kernel preserves `x_v * y_v = k` internally.

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
