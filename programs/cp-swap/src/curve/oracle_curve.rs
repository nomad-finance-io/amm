//! Oracle-anchored swap curve.
//!
//! Instead of letting the reserve ratio determine the quote price (as in
//! `x*y=k`), we anchor the marginal price to an external oracle (Pyth Lazer).
//! The actual pool reserves still back the trade — we just construct
//! *virtual reserves* centered on the oracle ratio and apply the constant-product
//! slippage formula on those.
//!
//! Given actual reserves `(x, y)` and oracle spot `P` (output per input), the
//! virtual reserves satisfy:
//!   - `y_v / x_v = P`         (marginal price matches oracle at zero size)
//!   - `x_v * y_v = x * y = k` (same liquidity depth as the actual pool)
//!
//! Solving gives `x_v = sqrt(k/P)` and `y_v = sqrt(k*P)`. The swap math is then
//! standard constant-product on the virtuals; the actual reserves update by the
//! real deltas, so accounting is unchanged.
//!
//! The bid/ask widening math (configured floor + age + confidence) lives
//! offchain in `client/src/oracle_math.rs` and is pushed onto PoolState by the
//! pool's keeper bot. This file only handles the on-chain consumption of the
//! cached bid/ask: raw-fraction conversion, inventory skew, and the virtual-
//! reserve swap math.

use crate::utils::U256;

/// Convert a Pyth Lazer price (mantissa + exponent) into an exact rational
/// `(num, den)` expressing the spot price in raw token units —
/// `raw_token_1 per raw_token_0`.
///
/// Pyth prices come as `mantissa * 10^exponent` in the *human* unit (e.g.
/// USD per SOL). To compare against pool reserves (which are in smallest-unit
/// u64s) we need to shift by the mint decimals:
/// ```text
/// P_raw = mantissa * 10^(exponent + mint_1_decimals - mint_0_decimals)
/// ```
///
/// # Errors
/// Returns `None` if the mantissa is non-positive, the effective power of ten
/// overflows a `u128`, or the numerator overflows during the multiplication.
pub fn pyth_price_to_raw_fraction(
    mantissa: i64,
    exponent: i16,
    mint_0_decimals: u8,
    mint_1_decimals: u8,
) -> Option<(u128, u128)> {
    if mantissa <= 0 {
        return None;
    }
    let m = mantissa as u128;

    let eff_exp = exponent as i32 + mint_1_decimals as i32 - mint_0_decimals as i32;

    if eff_exp >= 0 {
        let factor = 10u128.checked_pow(eff_exp as u32)?;
        let p_num = m.checked_mul(factor)?;
        Some((p_num, 1))
    } else {
        let factor = 10u128.checked_pow((-eff_exp) as u32)?;
        Some((m, factor))
    }
}

// -------------------------------------------------------------------------
// Inventory skew
// -------------------------------------------------------------------------
//
// Shifts the canonical oracle quote toward rebalancing when the pool's
// value-weighted inventory drifts from 50/50. Applied AFTER the offchain
// spread + Pyth bid/ask floor — so skew CAN push the quote inside Pyth's
// published market, up to `max_bps`. The cap is the adverse-selection
// bound: round-trip through skew should always cost more in fees + spread
// than it gives back as rebalancing incentive.
//
// Sign convention:
//   imbalance_bps: positive = token_0 surplus (value_0 > value_1)
//   skew_bps:      positive = raise canonical (mantissa goes up)
//                  negative = lower canonical
//   Token_0 surplus therefore produces NEGATIVE skew_bps (canonical down →
//   token_0 cheaper → encourages buying token_0 out of the pool).

/// Signed inventory imbalance in basis points, where
///     imbalance_bps = (value_0 - value_1) / (value_0 + value_1) × 10_000
/// and `value_0` is `reserve_0` priced in token_1 units via the oracle mid.
///
/// Range: `[-10_000, +10_000]`. `None` if the pool is empty or the oracle
/// fraction is degenerate.
pub fn compute_inventory_imbalance_bps(
    reserve_0_net: u64,
    reserve_1_net: u64,
    canonical_num: u128,
    canonical_den: u128,
) -> Option<i32> {
    if canonical_den == 0 {
        return None;
    }
    // Fast path: stay in u128 when the arithmetic fits — covers the vast
    // majority of pools (typical canonical_num ~ mantissa, well under 2^64).
    // Falls back to the U256 path on any overflow. The U256 path also handles
    // the legitimate empty-pool case (sum == 0) by returning None there too.
    if let Some(result) =
        imbalance_bps_u128(reserve_0_net, reserve_1_net, canonical_num, canonical_den)
    {
        return Some(result);
    }
    imbalance_bps_u256(reserve_0_net, reserve_1_net, canonical_num, canonical_den)
}

#[inline]
fn imbalance_bps_u128(
    reserve_0_net: u64,
    reserve_1_net: u64,
    canonical_num: u128,
    canonical_den: u128,
) -> Option<i32> {
    let r0 = reserve_0_net as u128;
    let r1 = reserve_1_net as u128;
    // value_0_in_token_1 = r0 * canonical_num / canonical_den.
    let prod = r0.checked_mul(canonical_num)?;
    let value_0 = prod / canonical_den;
    let sum = value_0.checked_add(r1)?;
    if sum == 0 {
        return None;
    }
    let (diff, sign) = if value_0 >= r1 {
        (value_0 - r1, 1i32)
    } else {
        (r1 - value_0, -1i32)
    };
    // diff * 10_000 may overflow u128 in extreme cases — let the caller
    // retry in U256 then.
    let scaled = diff.checked_mul(10_000u128)?;
    // Mathematically <= 10_000; clamp defensively.
    let magnitude = ((scaled / sum) as u64).min(10_000) as i32;
    Some(sign * magnitude)
}

#[cold]
fn imbalance_bps_u256(
    reserve_0_net: u64,
    reserve_1_net: u64,
    canonical_num: u128,
    canonical_den: u128,
) -> Option<i32> {
    let value_0 = U256::from(reserve_0_net)
        .checked_mul(U256::from(canonical_num))?
        .checked_div(U256::from(canonical_den))?;
    let value_1 = U256::from(reserve_1_net);

    let sum = value_0.checked_add(value_1)?;
    if sum.is_zero() {
        return None;
    }

    let (diff, sign) = if value_0 >= value_1 {
        (value_0 - value_1, 1i32)
    } else {
        (value_1 - value_0, -1i32)
    };

    let magnitude = diff.checked_mul(U256::from(10_000u64))?.checked_div(sum)?;

    let magnitude = magnitude.as_u64().min(10_000) as i32;
    Some(sign * magnitude)
}

/// Convert a signed imbalance into a signed skew shift to apply to the
/// canonical mantissa. Inside the dead zone returns 0; outside, scales
/// linearly with `bps_per_pct` bps of skew per 1% (= 100 bps) of imbalance
/// beyond the dead zone, clamped to ±`max_bps`.
pub fn compute_inventory_skew_bps(
    imbalance_bps: i32,
    deadzone_bps: u16,
    bps_per_pct: u16,
    max_bps: u16,
) -> i32 {
    let magnitude = imbalance_bps.unsigned_abs();
    let deadzone = deadzone_bps as u32;
    if magnitude <= deadzone {
        return 0;
    }
    let excess = magnitude - deadzone; // in basis points of imbalance
                                       // skew = excess / 100 * bps_per_pct  (1 pct = 100 bps of imbalance)
    let skew_mag =
        ((excess as u64).saturating_mul(bps_per_pct as u64) / 100).min(max_bps as u64) as i32;
    // Sign-flip: token_0 surplus (imbalance > 0) → canonical DOWN → negative skew.
    if imbalance_bps > 0 {
        -skew_mag
    } else {
        skew_mag
    }
}

/// Apply a signed bps shift to a mantissa:
///   result = mantissa × (10_000 + delta_bps) / 10_000
///
/// Returns `None` if the shift would produce a non-positive or over-large
/// mantissa (only reachable with extreme inputs).
pub fn shift_mantissa_by_signed_bps(mantissa: i64, delta_bps: i32) -> Option<i64> {
    if delta_bps == 0 {
        return Some(mantissa);
    }
    let m = mantissa as i128;
    let factor = 10_000_i128.checked_add(delta_bps as i128)?;
    if factor <= 0 {
        return None;
    }
    let result = m.checked_mul(factor)?.checked_div(10_000)?;
    if result <= 0 || result > i64::MAX as i128 {
        return None;
    }
    Some(result as i64)
}

/// Oracle-anchored swap curve.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OracleCurve;

impl OracleCurve {
    /// Given an input amount, return the output amount using the virtual-reserve
    /// method. The `spot` rate is already oriented as `output_per_input`.
    ///
    /// Returns `None` if any intermediate overflows, the pool is empty, or the
    /// computed output exceeds the actual output-side vault balance (pool can't
    /// service the trade at this anchor price).
    pub fn swap_base_input_without_fees(
        input_amount: u128,
        input_vault_amount: u128,
        output_vault_amount: u128,
        spot_num: u128,
        spot_den: u128,
    ) -> Option<u128> {
        if spot_num == 0 || spot_den == 0 {
            return None;
        }
        if input_vault_amount == 0 || output_vault_amount == 0 {
            return None;
        }

        let (x_v, y_v) =
            virtual_reserves(input_vault_amount, output_vault_amount, spot_num, spot_den)?;

        // Δy = y_v * Δx / (x_v + Δx)
        let delta_x = U256::from(input_amount);
        let new_x_v = x_v.checked_add(delta_x)?;
        let numerator = y_v.checked_mul(delta_x)?;
        let delta_y = numerator.checked_div(new_x_v)?;

        // Safety: can't pay out more than the actual pool holds. Happens when
        // the pool is imbalanced against the oracle and someone tries to drain
        // the short side. Caller should reject.
        if delta_y >= U256::from(output_vault_amount) {
            return None;
        }

        Some(delta_y.as_u128())
    }

    /// Given a desired output amount, return the input amount required
    /// (ceiling-rounded in the pool's favor). `spot` is `output_per_input`.
    pub fn swap_base_output_without_fees(
        output_amount: u128,
        input_vault_amount: u128,
        output_vault_amount: u128,
        spot_num: u128,
        spot_den: u128,
    ) -> Option<u128> {
        if spot_num == 0 || spot_den == 0 {
            return None;
        }
        if input_vault_amount == 0 || output_vault_amount == 0 {
            return None;
        }

        let (x_v, y_v) =
            virtual_reserves(input_vault_amount, output_vault_amount, spot_num, spot_den)?;

        let delta_y = U256::from(output_amount);
        // Can't extract the entire virtual output (denominator would hit zero
        // or flip). Also guard against exceeding actual output reserves.
        if delta_y >= y_v || output_amount >= output_vault_amount {
            return None;
        }

        // Δx = ceil(x_v * Δy / (y_v - Δy)) — pool-favorable rounding
        let numerator = x_v.checked_mul(delta_y)?;
        let denominator = y_v.checked_sub(delta_y)?;
        let q = numerator.checked_div(denominator)?;
        let r = numerator.checked_rem(denominator)?;
        let delta_x = if r.is_zero() {
            q
        } else {
            q.checked_add(U256::one())?
        };

        if delta_x > U256::from(u128::MAX) {
            return None;
        }
        Some(delta_x.as_u128())
    }
}

/// Compute virtual reserves `(x_v, y_v)` anchored to the oracle spot rate.
///
/// - `x_v = sqrt(input_vault * output_vault * spot_den / spot_num)`
/// - `y_v = x_v * spot_num / spot_den`         (algebraic identity: y_v/x_v = P)
///
/// Uses U256 to avoid overflow when the full product is computed before the
/// divide. Only one `integer_sqrt` is taken; the second virtual reserve is
/// derived multiplicatively, saving one Newton iteration loop per swap.
/// Integer truncation: ≤1 unit on the sqrt side, ≤1 additional unit from the
/// derived divide — both pool-favorable on the output side.
fn virtual_reserves(
    input_vault_amount: u128,
    output_vault_amount: u128,
    spot_num: u128,
    spot_den: u128,
) -> Option<(U256, U256)> {
    let xy = U256::from(input_vault_amount).checked_mul(U256::from(output_vault_amount))?;
    let spot_num_u = U256::from(spot_num);
    let spot_den_u = U256::from(spot_den);

    let x_v = xy
        .checked_mul(spot_den_u)?
        .checked_div(spot_num_u)?
        .integer_sqrt();
    let y_v = x_v.checked_mul(spot_num_u)?.checked_div(spot_den_u)?;

    Some((x_v, y_v))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a fraction directly without going through Pyth normalization.
    /// Useful when tests want to set a specific raw spot rate.
    fn spot(num: u128, den: u128) -> (u128, u128) {
        (num, den)
    }

    const SOL_MID: i64 = 16_000_000_000; // SOL at $160 with exponent -8

    #[test]
    fn pyth_normalization_sol_usdc() {
        // SOL/USD at $160 with exponent -8: mantissa = 16_000_000_000
        // Pool: token_0 = SOL (9 decimals), token_1 = USDC (6 decimals)
        // Expected raw ratio: 0.16 raw_usdc per raw_sol => 160e8 / 1e11
        let (num, den) = pyth_price_to_raw_fraction(16_000_000_000, -8, 9, 6).unwrap();
        // 160e8 / 1e11 == 0.16, represented exactly as 16_000_000_000 / 100_000_000_000
        assert_eq!(num, 16_000_000_000);
        assert_eq!(den, 100_000_000_000);
    }

    #[test]
    fn pyth_normalization_rejects_zero_mantissa() {
        assert!(pyth_price_to_raw_fraction(0, -8, 9, 6).is_none());
        assert!(pyth_price_to_raw_fraction(-1, -8, 9, 6).is_none());
    }

    #[test]
    fn balanced_pool_matches_constant_product() {
        // Pool at oracle ratio: 10 SOL + 1600 USDC at 160 USDC/SOL.
        // In raw units: x = 10e9 lamports, y = 1.6e9 raw USDC.
        let x = 10_000_000_000u128;
        let y = 1_600_000_000u128;
        // Spot (in raw units) = y/x = 0.16. Express as 16e8/1e11.
        let (num, den) = pyth_price_to_raw_fraction(16_000_000_000, -8, 9, 6).unwrap();

        // Small buy of 0.01 SOL worth of USDC → 0.1 raw SOL? no wait
        // ZeroForOne: input = raw SOL, output = raw USDC.
        let delta_x = 100_000_000u128; // 0.1 SOL in lamports
        let oracle_out =
            OracleCurve::swap_base_input_without_fees(delta_x, x, y, num, den).unwrap();

        // Pure x*y=k reference: Δy = y*Δx/(x+Δx) = 1.6e9 * 1e8 / (1.01e10) ≈ 15_841_584
        let k = x * y;
        let pure_out = y - k / (x + delta_x);

        // Balanced pool: virtual == actual, so oracle kernel must match pure x*y=k
        // (ignoring a ≤1-lamport integer-sqrt rounding).
        assert!(
            oracle_out.abs_diff(pure_out) <= 1,
            "oracle={oracle_out} pure={pure_out}"
        );
    }

    #[test]
    fn imbalanced_pool_quotes_near_oracle_not_reserve_ratio() {
        // Setup from earlier discussion:
        //   Pool: 10 SOL + 1000 USDC (reserve ratio says $100/SOL)
        //   Oracle: $160/SOL
        //   User buys 1 SOL with USDC (OneForZero).
        //
        // Pure x*y=k would cost ~111 USDC (way below market).
        // Oracle-anchored should cost ~184 USDC (close to market + slippage).
        let token_0_sol = 10_000_000_000u128; // 10 SOL
        let token_1_usdc = 1_000_000_000u128; // 1000 USDC
        let (p_num, p_den) = pyth_price_to_raw_fraction(16_000_000_000, -8, 9, 6).unwrap();

        // OneForZero: input=USDC, output=SOL => spot = token_0/token_1 = inverse of P
        // We want Δ_input such that Δ_output = 1 SOL. Use base_output.
        let desired_sol_out = 1_000_000_000u128; // 1 SOL

        let usdc_in_oracle = OracleCurve::swap_base_output_without_fees(
            desired_sol_out,
            token_1_usdc, // input_vault = USDC
            token_0_sol,  // output_vault = SOL
            p_den,        // inverted: output_per_input = SOL/USDC
            p_num,
        )
        .unwrap();

        // Expect ~184 USDC (=184_000_000 raw), within a few percent.
        assert!(
            usdc_in_oracle > 170_000_000 && usdc_in_oracle < 200_000_000,
            "imbalanced oracle quote should cost close to market ($160-$200), got {usdc_in_oracle}"
        );

        // And definitely NOT the pure x*y=k answer (~111 USDC).
        assert!(
            usdc_in_oracle > 150_000_000,
            "oracle-anchored should not price at the stale reserve ratio, got {usdc_in_oracle}"
        );
    }

    #[test]
    fn swap_base_input_then_output_roundtrip() {
        // If swap_base_input says Δx → Δy, then swap_base_output with Δy
        // should return an input very close to Δx (equal up to ceiling).
        let x = 10_000_000_000u128;
        let y = 1_600_000_000u128;
        let (num, den) = spot(16, 100);

        let delta_x = 50_000_000u128;
        let delta_y = OracleCurve::swap_base_input_without_fees(delta_x, x, y, num, den).unwrap();
        let delta_x_back =
            OracleCurve::swap_base_output_without_fees(delta_y, x, y, num, den).unwrap();

        // Roundtrip should be within 1 unit (ceiling vs floor on same math).
        assert!(
            delta_x.abs_diff(delta_x_back) <= 1,
            "roundtrip: forward delta_x={delta_x}, back={delta_x_back}"
        );
    }

    #[test]
    fn zero_spot_rejected() {
        let x = 10_000_000u128;
        let y = 10_000_000u128;
        assert!(OracleCurve::swap_base_input_without_fees(1000, x, y, 0, 1).is_none());
        assert!(OracleCurve::swap_base_input_without_fees(1000, x, y, 1, 0).is_none());
    }

    #[test]
    fn empty_vault_rejected() {
        let (num, den) = spot(1, 1);
        assert!(OracleCurve::swap_base_input_without_fees(100, 0, 1000, num, den).is_none());
        assert!(OracleCurve::swap_base_input_without_fees(100, 1000, 0, num, den).is_none());
    }

    #[test]
    fn swap_too_large_for_actual_reserves_rejected() {
        // Pool: 10_000 token_0, 10 token_1. Oracle spot = 1:1.
        // Virtual reserves would be ≈ (316.2, 316.2). A small Δx would want
        // to pay out more token_1 than the pool actually has.
        let x = 10_000u128;
        let y = 10u128;
        let (num, den) = spot(1, 1);
        // Virtual y_v ~316. Δx=20 → Δy ~ 316*20/(316+20) ~ 18.8. But actual y=10.
        assert!(OracleCurve::swap_base_input_without_fees(20, x, y, num, den).is_none());
    }

    // ---- inventory skew ------------------------------------------------------

    /// Build a canonical price fraction matching SOL/USDC mint decimals (9, 6)
    /// at $160 mid. `value_0_in_token_1 = reserve_0_lamports * num / den`.
    fn sol_usdc_rate() -> (u128, u128) {
        pyth_price_to_raw_fraction(SOL_MID, -8, 9, 6).unwrap()
    }

    #[test]
    fn imbalance_balanced_pool_is_zero() {
        // Oracle says $160/SOL. 10 SOL @ $160 = 1600 USDC of value on each side.
        let (num, den) = sol_usdc_rate();
        let imb = compute_inventory_imbalance_bps(
            10_000_000_000, // 10 SOL in lamports
            1_600_000_000,  // 1600 USDC raw
            num,
            den,
        )
        .unwrap();
        assert!(imb.abs() <= 1, "expected ~0, got {imb}");
    }

    #[test]
    fn imbalance_token_0_surplus_positive() {
        // 15 SOL ($2400) + 800 USDC → v0=2400, v1=800, imbalance = +1600/3200 = +5000 bps
        let (num, den) = sol_usdc_rate();
        let imb = compute_inventory_imbalance_bps(15_000_000_000, 800_000_000, num, den).unwrap();
        assert_eq!(imb, 5_000);
    }

    #[test]
    fn imbalance_token_1_surplus_negative() {
        // 5 SOL ($800) + 2400 USDC → v0=800, v1=2400, imbalance = -1600/3200 = -5000 bps
        let (num, den) = sol_usdc_rate();
        let imb = compute_inventory_imbalance_bps(5_000_000_000, 2_400_000_000, num, den).unwrap();
        assert_eq!(imb, -5_000);
    }

    #[test]
    fn imbalance_empty_pool_rejected() {
        let (num, den) = sol_usdc_rate();
        assert!(compute_inventory_imbalance_bps(0, 0, num, den).is_none());
    }

    #[test]
    fn imbalance_u128_and_u256_paths_agree() {
        // Sanity: both fast and slow paths should produce identical results
        // on inputs that fit comfortably in u128.
        let (num, den) = sol_usdc_rate();
        let cases = [
            (10_000_000_000u64, 1_600_000_000u64),
            (15_000_000_000u64, 800_000_000u64),
            (5_000_000_000u64, 2_400_000_000u64),
            (1u64, 1u64),
            (u64::MAX, 1u64),
        ];
        for (r0, r1) in cases {
            let fast = imbalance_bps_u128(r0, r1, num, den).unwrap();
            let slow = imbalance_bps_u256(r0, r1, num, den).unwrap();
            assert_eq!(fast, slow, "fast/slow mismatch for r0={r0}, r1={r1}");
        }
    }

    #[test]
    fn imbalance_u256_fallback_handles_overflow() {
        // canonical_num near u128::MAX forces r0 * canonical_num to overflow
        // u128, which the public function should silently route to U256.
        // Arbitrary but extreme: canonical_num such that r0 * num > 2^128.
        let r0 = u64::MAX; // ~2^64
        let r1 = u64::MAX; // ~2^64
        let canonical_num = u128::MAX / 2; // ~2^127
        let canonical_den = 1u128;
        // r0 * canonical_num ≈ 2^191 → fast path overflow → U256 fallback engages.
        let imb = compute_inventory_imbalance_bps(r0, r1, canonical_num, canonical_den).unwrap();
        // value_0 ≈ r0 * canonical_num >> r1, so imbalance should be ~+10_000.
        assert!(
            imb > 9_000,
            "expected near-saturation positive imbalance, got {imb}"
        );
    }

    #[test]
    fn skew_inside_deadzone_is_zero() {
        // deadzone 300 bps → any |imbalance| <= 300 returns 0.
        assert_eq!(compute_inventory_skew_bps(0, 300, 1, 50), 0);
        assert_eq!(compute_inventory_skew_bps(100, 300, 1, 50), 0);
        assert_eq!(compute_inventory_skew_bps(-300, 300, 1, 50), 0);
    }

    #[test]
    fn skew_linear_outside_deadzone() {
        // deadzone 300 bps, 1 bps of skew per 1% imbalance (= per 100 bps imbalance).
        // At imbalance = 500 bps → excess = 200 → skew = 200/100 * 1 = 2 bps
        // Positive imbalance → negative skew (rebalance down).
        assert_eq!(compute_inventory_skew_bps(500, 300, 1, 50), -2);
        assert_eq!(compute_inventory_skew_bps(-500, 300, 1, 50), 2);

        // At imbalance = 2_300 bps → excess = 2_000 → skew = 20 bps
        assert_eq!(compute_inventory_skew_bps(2_300, 300, 1, 50), -20);
    }

    #[test]
    fn skew_clamps_to_max() {
        // At imbalance = 10_000 bps, bps_per_pct=1 would give 97 bps; max=50 caps it.
        assert_eq!(compute_inventory_skew_bps(10_000, 300, 1, 50), -50);
        assert_eq!(compute_inventory_skew_bps(-10_000, 300, 1, 50), 50);
    }

    #[test]
    fn skew_symmetric_under_token_swap() {
        // Imbalance +X should produce skew equal in magnitude but opposite in sign
        // to imbalance -X. Critical property — catches sign-flip bugs.
        for imb in [500i32, 1_234, 5_000, 9_999] {
            let pos = compute_inventory_skew_bps(imb, 300, 1, 50);
            let neg = compute_inventory_skew_bps(-imb, 300, 1, 50);
            assert_eq!(pos, -neg, "asymmetric at imbalance {imb}");
        }
    }

    #[test]
    fn skew_bps_per_pct_scales_linearly() {
        // At imbalance 1_300 bps (excess 1_000 = 10%):
        //   bps_per_pct=1  → 10 bps
        //   bps_per_pct=2  → 20 bps
        //   bps_per_pct=10 → 100 bps (but capped by max)
        assert_eq!(compute_inventory_skew_bps(1_300, 300, 1, 500), -10);
        assert_eq!(compute_inventory_skew_bps(1_300, 300, 2, 500), -20);
        assert_eq!(compute_inventory_skew_bps(1_300, 300, 10, 500), -100);
    }

    #[test]
    fn shift_mantissa_up_by_positive_bps() {
        // 100 bps = +1% → mantissa 16_000_000_000 → 16_160_000_000
        assert_eq!(
            shift_mantissa_by_signed_bps(16_000_000_000, 100),
            Some(16_160_000_000)
        );
    }

    #[test]
    fn shift_mantissa_down_by_negative_bps() {
        assert_eq!(
            shift_mantissa_by_signed_bps(16_000_000_000, -100),
            Some(15_840_000_000)
        );
    }

    #[test]
    fn shift_mantissa_zero_delta_is_identity() {
        assert_eq!(shift_mantissa_by_signed_bps(12_345, 0), Some(12_345));
    }

    #[test]
    fn shift_mantissa_rejects_flip_to_nonpositive() {
        // delta = -10_000 bps → factor 0 → result 0 → rejected.
        assert!(shift_mantissa_by_signed_bps(16_000_000_000, -10_000).is_none());
        // delta = -10_001 bps → factor -1 → rejected.
        assert!(shift_mantissa_by_signed_bps(16_000_000_000, -10_001).is_none());
    }
}
