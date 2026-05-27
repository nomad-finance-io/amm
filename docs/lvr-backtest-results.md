# LVR Backtest — Nomad Oracle AMM vs Orca Whirlpool (SOL/USDC)

**Status:** Empirical backtest against historical on-chain swaps.
**Last updated:** 2026-05-10

## TL;DR

Over **7.0 days** of real Orca SOL/USDC 0.04% Whirlpool flow
(0 swaps, $0.0M total volume), an
oracle-anchored AMM with 5 bps full spread would have netted LPs
**$0** more than Orca did over the same window — a
**0.0%** improvement in LP P&L from order flow.

Annualized: **$0/yr** of LP savings on a pool of this size.

## Method

For each historical swap on `Czfq3xZZDmsdGdUyrNLtRhGc47cXcZtL3K9DrduHFwR4`:

1. **Orca actual** — record the on-chain executed price.
2. **Reference price** — Binance SOLUSDC 1-minute close at the swap timestamp,
   and again at *t* + 60s (markout horizon).
3. **Orca markout P&L** = `pool_sol_delta × (ref_after − executed_price)`
   (signed: negative = LP got adversely selected).
4. **Orca fee revenue** = `4 bps × notional`.
5. **Nomad quote** = `ref_now × (1 ± 2.5 bps)`, side chosen by trade direction.
6. **Nomad markout P&L** = `pool_sol_delta × (ref_after − nomad_quote)` —
   spread is already inside the quote, so this is the LP's *net* P&L.

LP savings = `nomad_net − orca_net`.

## Results

| Metric | Orca (actual) | Nomad (counterfactual) |
|---|---:|---:|
| Swaps replayed | 0 | 0 |
| Total volume | $0.00M | $0.00M |
| Markout P&L | $0 | $0 |
| Fee revenue | $0 | (in spread) |
| **Net LP P&L** | **$0** | **$0** |

| | Value |
|---|---:|
| LP savings (7.0d) | **$0** |
| LP savings (annualized) | **$0/yr** |
| Improvement vs Orca | **0.0%** |

Per-day breakdown: `docs/lvr-backtest-daily.csv`.

## Window

- Start: 2026-05-03 00:00 UTC
- End:   2026-05-10 00:00 UTC
- Pool:  `Czfq3xZZDmsdGdUyrNLtRhGc47cXcZtL3K9DrduHFwR4` (Orca SOL/USDC, 4 bps tier)
- Reference: Binance SOLUSDC 1-minute close, 60s markout horizon
- Nomad spread: 5 bps full (matches `min_spread_bps` config)

## Caveats

- **Counterfactual flow assumption.** We assume the same order flow lands on
  Nomad. In practice, traders route to the best price; if Nomad quotes worse
  than Orca on some trades, that flow would route elsewhere. The number
  therefore over-states the volume Nomad would actually capture.
- **Spread-only Nomad model.** Quote at `ref ± spread/2`, ignoring Nomad's
  virtual-reserve slippage and dynamic widening. For typical retail-sized
  trades these are negligible; for whale trades they would *protect* LPs more
  than this model shows. So the headline is conservative on the Nomad side.
- **Stale-oracle cost not modeled.** Assumes Nomad's oracle is fresh at the
  trade timestamp. Real keeper lag would add small LVR back. This is the
  largest unmodeled cost on Nomad's side.
- **Reference price granularity.** 1-minute Binance close is a proxy for
  continuous mid-market. Sub-minute price moves are smoothed.
- **Markout horizon.** 60s is the standard CEX-DEX adverse-selection
  window. Shorter horizons reduce measured LVR; longer ones include drift
  unrelated to this trade.
- **Excludes Orca incentive rewards.** Orca LPs may earn additional ORCA token
  emissions; not netted here.
- **Excludes Nomad gas/keeper costs.** Operating Nomad's oracle keeper has
  ongoing cost not netted here.

## Files

- `scripts/lvr-backtest/backtest.py` — script
- `scripts/lvr-backtest/data/` — cached fetches (gitignored)
- `docs/lvr-backtest-daily.csv` — per-day breakdown
