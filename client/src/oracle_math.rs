//! Offchain spread-widening math.
//!
//! These helpers compute the effective bid / ask mantissas a keeper bot
//! should push to a pool's `update_pool_oracle` instruction. They are pure
//! integer arithmetic with no external dependencies, so the same crate can
//! be linked into a Rust bot via `client = { path = "../client" }`.
//!
//! Pipeline:
//!   1. `compute_confidence_adjusted_min_spread_bps` widens the configured
//!      floor by Pyth confidence.
//!   2. `compute_effective_bid_ask_mantissas` derives bid / ask mantissas
//!      from the price + widened floor + (optional) Pyth bid / ask.
//!   3. The bot pushes those mantissas (plus its own dynamic fee) on-chain.

/// Compute effective bid / ask mantissas for the pool's quote.
///
/// Returns `(bid_mantissa, ask_mantissa)` in the same mantissa + exponent
/// convention as `price_mantissa` (canonical token_1 / token_0 direction), with
/// `bid <= price <= ask`.
///
/// Semantics:
/// - `min_spread_bps` is the **total** configured spread in basis points
///   (e.g. `40` = 40 bps total = 20 bps each side of mid).
/// - Pyth's published `best_bid_mantissa` / `best_ask_mantissa` are used as
///   additional floors — when the market signals a wider spread than the
///   configured minimum, defer to the market. When tighter or absent, hold
///   the configured floor. This protects LPs during calm markets (tight Pyth
///   spreads pick off the pool via adverse selection) while inheriting
///   automatic widening during volatility.
///
/// # Errors
/// - Non-positive `price_mantissa`
/// - Intermediate i128 overflow
/// - Any side's effective mantissa leaves the i64 range
/// - Final bid <= 0 or bid > ask (caught by the trailing sanity check; would
///   only happen if `min_spread_bps` were somehow pushed past the mathematical
///   cliff at 20_000, which init validation already prevents)
pub fn compute_effective_bid_ask_mantissas(
    price_mantissa: i64,
    best_bid_mantissa: Option<i64>,
    best_ask_mantissa: Option<i64>,
    min_spread_bps: u16,
) -> Option<(i64, i64)> {
    if price_mantissa <= 0 {
        return None;
    }
    let bps = min_spread_bps as i128;
    let price = price_mantissa as i128;

    // Min-spread floors computed from mid: half the total spread on each side.
    // ask_floor = price * (20_000 + bps) / 20_000
    // bid_floor = price * (20_000 - bps) / 20_000
    // The 20_000 denominator is a mathematical constant: 10_000 (bps) × 2
    // (half-spread per side). It encodes the convention, not a policy cap.
    let ask_floor = price.checked_mul(20_000_i128 + bps)?.checked_div(20_000)?;
    let bid_floor = price.checked_mul(20_000_i128 - bps)?.checked_div(20_000)?;

    // Widen (never tighten) to Pyth's published bid/ask when they exist.
    let bid = match best_bid_mantissa {
        Some(b) if (b as i128) < bid_floor && b > 0 => b as i128,
        _ => bid_floor,
    };
    let ask = match best_ask_mantissa {
        Some(a) if (a as i128) > ask_floor && a > 0 => a as i128,
        _ => ask_floor,
    };

    if bid <= 0 || ask <= 0 || bid > ask {
        return None;
    }
    if bid > i64::MAX as i128 || ask > i64::MAX as i128 {
        return None;
    }

    Some((bid as i64, ask as i64))
}

/// Apply a gentle confidence-driven bump to the configured minimum spread.
/// Confidence is interpreted as basis points of price and capped to avoid
/// making the pool non-competitive during extreme or pathological payloads.
pub const CONFIDENCE_SPREAD_MULTIPLIER: u64 = 1;
pub const MAX_CONFIDENCE_SPREAD_BPS: u64 = 20;

pub fn compute_confidence_adjusted_min_spread_bps(
    min_spread_bps: u16,
    confidence_mantissa: Option<i64>,
    price_mantissa: i64,
) -> u16 {
    if price_mantissa <= 0 {
        return min_spread_bps.saturating_add(MAX_CONFIDENCE_SPREAD_BPS as u16);
    }

    let confidence_spread_bps: u64 = match confidence_mantissa {
        Some(c) if c > 0 => {
            let raw_bps = (c as u128)
                .saturating_mul(10_000)
                .checked_div(price_mantissa as u128)
                .unwrap_or(0);
            raw_bps
                .saturating_mul(CONFIDENCE_SPREAD_MULTIPLIER as u128)
                .min(MAX_CONFIDENCE_SPREAD_BPS as u128) as u64
        }
        _ => 0,
    };

    min_spread_bps.saturating_add(confidence_spread_bps as u16)
}
#[cfg(test)]
mod tests {
    use super::*;

    const SOL_MID: i64 = 16_000_000_000; // SOL at $160 with exponent -8
    const SOL_1_BPS: i64 = 1_600_000; // 1 bps of SOL price in mantissa units

    // ---- compute_effective_bid_ask_mantissas ---------------------------------

    #[test]
    fn spread_floor_only_no_oracle_bid_ask() {
        // price 160 (mantissa 16e9 at exp -8), 40 bps total spread (20 each side).
        // Expect: bid = 160 * 0.998 = 159.68, ask = 160 * 1.002 = 160.32.
        let (bid, ask) =
            compute_effective_bid_ask_mantissas(16_000_000_000, None, None, 40).unwrap();
        assert_eq!(bid, 15_968_000_000);
        assert_eq!(ask, 16_032_000_000);
    }

    #[test]
    fn spread_oracle_wider_than_floor_is_used() {
        // Min floor at 20 bps: bid=159.68, ask=160.32.
        // Oracle publishes a wider market: bid=159.00, ask=161.00.
        // Expect the oracle values to win (pool protects itself).
        let (bid, ask) = compute_effective_bid_ask_mantissas(
            16_000_000_000,
            Some(15_900_000_000),
            Some(16_100_000_000),
            40,
        )
        .unwrap();
        assert_eq!(bid, 15_900_000_000);
        assert_eq!(ask, 16_100_000_000);
    }

    #[test]
    fn spread_oracle_tighter_than_floor_is_ignored() {
        // Min floor at 40 bps total: bid=159.68, ask=160.32.
        // Oracle says bid=159.99, ask=160.01 (near zero spread).
        // Expect the floor to hold.
        let (bid, ask) = compute_effective_bid_ask_mantissas(
            16_000_000_000,
            Some(15_999_000_000),
            Some(16_001_000_000),
            40,
        )
        .unwrap();
        assert_eq!(bid, 15_968_000_000);
        assert_eq!(ask, 16_032_000_000);
    }

    #[test]
    fn spread_mixed_sides_one_tight_one_wide() {
        // Oracle bid is wider than floor; oracle ask is tighter than floor.
        // Expect: bid from oracle, ask from floor.
        let (bid, ask) = compute_effective_bid_ask_mantissas(
            16_000_000_000,
            Some(15_800_000_000), // wider than floor (15.968e9)
            Some(16_001_000_000), // tighter than floor (16.032e9)
            40,
        )
        .unwrap();
        assert_eq!(bid, 15_800_000_000);
        assert_eq!(ask, 16_032_000_000);
    }

    #[test]
    fn spread_zero_bps_with_no_oracle_collapses_to_mid() {
        // zero spread + no oracle bid/ask → both sides == price.
        // Kernel would then quote at mid (not useful in prod, but not rejected here).
        let (bid, ask) =
            compute_effective_bid_ask_mantissas(16_000_000_000, None, None, 0).unwrap();
        assert_eq!(bid, 16_000_000_000);
        assert_eq!(ask, 16_000_000_000);
    }

    #[test]
    fn spread_invalid_inputs_rejected() {
        // Non-positive price.
        assert!(compute_effective_bid_ask_mantissas(0, None, None, 40).is_none());
        assert!(compute_effective_bid_ask_mantissas(-1, None, None, 40).is_none());
        // Runaway bps.
        assert!(compute_effective_bid_ask_mantissas(16_000_000_000, None, None, 20_000).is_none());
    }

    #[test]
    fn spread_ignores_non_positive_oracle_side() {
        // If the oracle publishes a non-positive (garbage) bid, fall back to the floor.
        let (bid, _ask) =
            compute_effective_bid_ask_mantissas(16_000_000_000, Some(0), None, 40).unwrap();
        assert_eq!(bid, 15_968_000_000);
    }

    // ---- confidence widening -------------------------------------------------

    #[test]
    fn confidence_spread_no_confidence_unchanged() {
        assert_eq!(
            compute_confidence_adjusted_min_spread_bps(40, None, SOL_MID),
            40
        );
        assert_eq!(
            compute_confidence_adjusted_min_spread_bps(40, Some(0), SOL_MID),
            40
        );
    }

    #[test]
    fn confidence_spread_calm_market_small_bump() {
        // 1 bps confidence on $160 SOL → +1 total spread bps at 1x multiplier.
        assert_eq!(
            compute_confidence_adjusted_min_spread_bps(40, Some(SOL_1_BPS), SOL_MID),
            41
        );
    }

    #[test]
    fn confidence_spread_mild_volatility() {
        // 10 bps confidence → +10 spread bps.
        assert_eq!(
            compute_confidence_adjusted_min_spread_bps(40, Some(10 * SOL_1_BPS), SOL_MID),
            50
        );
    }

    #[test]
    fn confidence_spread_caps_gently() {
        // 50 bps confidence would imply +50 bps, but we cap at +20.
        assert_eq!(
            compute_confidence_adjusted_min_spread_bps(40, Some(50 * SOL_1_BPS), SOL_MID),
            60
        );
        assert_eq!(
            compute_confidence_adjusted_min_spread_bps(40, Some(i64::MAX), SOL_MID),
            60
        );
    }

    #[test]
    fn confidence_spread_non_positive_price_defensive_cap() {
        assert_eq!(
            compute_confidence_adjusted_min_spread_bps(40, Some(SOL_1_BPS), 0),
            60
        );
        assert_eq!(
            compute_confidence_adjusted_min_spread_bps(40, Some(SOL_1_BPS), -1),
            60
        );
    }
}
