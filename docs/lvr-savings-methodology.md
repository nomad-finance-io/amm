# LVR Savings Methodology — Nomad Oracle AMM vs CLMM

**Status:** Draft v0.1 — model-driven estimate, not validated against on-chain measurement.
**Last updated:** 2026-05-09

This document estimates how much Loss-Versus-Rebalancing (LVR) the Nomad Oracle AMM saves
LPs compared to a traditional concentrated-liquidity AMM (Orca Whirlpool, used as the
real-world benchmark) under the assumption of one Solana slot (~400ms) of oracle
staleness.

## TL;DR

At base-case parameters (SOL/USDC, σ = 70%, L_eff = 3, τ = 400ms, s = 5 bps), Nomad
saves LPs **~10.9 percentage points of APR** in LVR drag versus a comparable Whirlpool
position — roughly **$3.2M/yr** on a pool sized to Orca's dominant SOL/USDC pair
($29.37M TVL).

Defensible range across reasonable parameters: **5–14 pp APR**.

The largest single source of model uncertainty is a 30× "real-world correction"
multiplier applied to the idealized residual LVR. That number is engineering judgment,
not measurement. Replace it the moment you have devnet/canary telemetry.

---

## 1. Inputs

| Input | Value | Source |
|---|---|---|
| TVL | $29,373,327 | Orca SOL/USDC, 2026-05-09 20:00 UTC |
| 24h volume | $60,907,431 | same |
| Daily turnover | 207% | derived |
| Pair | SOL/USDC | — |
| Annualized vol σ | 70% mid (60–80% bracket) | recent SOL realized vol; **measure to validate** |
| Oracle staleness τ | 400ms (1 Solana slot) | conservative assumption |
| Oracle AMM full spread `s` | 5 bps (= 2.5 bps half-spread) | `min_spread_bps = 5` config |
| CLMM fee tier `γ` (Whirlpool) | 5 bps | typical SOL/USDC dominant pool |
| Year (seconds) | 31,557,600 | — |

## 2. Assumptions (load-bearing)

These are the assumptions that drive the result. Flag any that look wrong before
quoting numbers externally.

1. **Price process is geometric Brownian motion.** Standard LVR framework
   (Milionis-Moallemi-Roughgarden 2022). Real prices have jumps and fat tails;
   this understates LVR in vol-spike regimes.
2. **Oracle keeper updates every slot.** No tail latency. Empirically false —
   keeper failures and slot skipping happen. Quantified separately in the 30×
   correction multiplier.
3. **CLMM pool's effective leverage L_eff = 3.** Most concentrated liquidity
   sits near spot; aggregate position behaves like a CPMM at 3× capital efficiency.
   Range tested: 2–5.
4. **Spread is the only mechanism gating arb on the oracle AMM.** Inventory skew
   and dynamic spread widening from `compute_dynamic_min_spread_bps` are
   conservatively ignored — they would *improve* the oracle AMM's number further.
5. **Symmetric two-way flow.** Volume splits 50/50 buy/sell; no persistent
   inventory drift.
6. **No MEV / private orderflow advantage** beyond what's baked into the σ
   assumption.
7. **CLMM fee offset is 40% of gross LVR** at 5 bps tier and 207% turnover.
   Rule-of-thumb from M-M-R-T 2023 with-fees framework; should be solved
   numerically for production estimates.

## 3. Step 1 — Baseline LVR rates

### Vanilla CPMM (Milionis-Moallemi-Roughgarden 2022)

```
LVR_CPMM = σ²/8        (annualized, fraction of TVL)
        = 0.70²/8
        = 6.13% APR
```

### CLMM with effective leverage L_eff

```
LVR_CLMM_gross = L_eff × σ²/8
              = 3 × 6.13%
              = 18.4% APR  (gross, before fees)
```

### Fee offset on CLMM (M-M-R-T 2023)

At 5 bps fee and σ_per_block = σ × √(τ/T_year) = 0.79 bps, the fee covers a
3.17σ block move. Closed-form fee offset for high-turnover pools is
~30–50% of gross LVR. Take 40% as the mid:

```
LVR_CLMM_net ≈ 18.4% × (1 − 0.40)
            = 11.0% APR
```

This is the LVR drag a CLMM LP actually pays after fee revenue offsets it.

## 4. Step 2 — Oracle AMM residual LVR

### Per-window stddev

```
σ_τ = σ × √(τ / T_year)
    = 0.70 × √(0.4 / 31,557,600)
    = 0.79 bps
```

### Arb threshold

The oracle AMM only loses to arbs when |move during τ| > half-spread:

```
z = (s/2) / σ_τ
  = 2.5 / 0.79
  = 3.17
```

### Leakage function

Expected excess move past threshold, normalized by E[|Z|] = √(2/π):

```
g(z)    = 2[φ(z) − z(1 − Φ(z))] / √(2/π)
g(3.17) = 2[0.00262 − 3.17 × 0.000762] / 0.798
        = 0.063%
```

### Idealized residual LVR

```
LVR_oracle_idealized = (σ²/8) × g(z)
                    = 6.13% × 0.063%
                    = 0.0039% APR
```

### Real-world correction multiplier

The idealized model misses three real-world effects. Conservative size estimates:

| Effect | Multiplier | Reasoning |
|---|---|---|
| Keeper lag tail | 3× | P95 staleness ~2–3 slots vs assumed 1 |
| Toxic informed flow | 5× | CEX-DEX latency arb not in σ²/8 framework |
| Vol regime spikes | 2× | Instantaneous σ → 2× during news events |
| **Combined** | **30×** | Roughly multiplicative |

```
LVR_oracle_realistic ≈ 0.0039% × 30
                    ≈ 0.12% APR
```

The 30× combined multiplier is the squishiest number in the document. It is
engineering judgment, not data.

## 5. Step 3 — Net LVR savings

| Metric | CLMM (Orca) | Oracle AMM (Nomad) |
|---|---|---|
| Gross LVR | 18.4% APR | 0.0039% APR |
| Fee/spread offset | 40% (5 bps fee) | n/a — spread is LP revenue, accounted separately |
| **Net LVR drag (idealized)** | **11.0% APR** | **0.0039% APR** |
| Real-world adjustment | already in fee offset | × 30 corrections |
| **Net LVR drag (realistic)** | **11.0% APR** | **~0.12% APR** |

**LVR savings to LPs (Nomad vs Orca): ~10.9 percentage points APR**
**Dollar terms on $29.37M TVL: ~$3.2M/yr**

## 6. Sensitivity table

| σ | L_eff | τ | CLMM net LVR | Oracle residual | **Savings (pp APR)** |
|---|---|---|---|---|---|
| 60% | 2 | 0.4s | 5.4% | 0.01% | **~5.4** |
| 70% | 3 | 0.4s | 11.0% | 0.12% | **~10.9** ← base case |
| 80% | 3 | 0.4s | 14.4% | 0.49% | **~13.9** |
| 70% | 5 | 0.4s | 18.4% | 0.12% | **~18.3** |
| 70% | 3 | 2.0s | 11.0% | 0.30% | **~10.7** |

The base case sits at ~11 pp APR savings, with a defensible range of **5–14 pp** across
reasonable parameter choices.

## 7. What's load-bearing, and how to validate

| Parameter | Risk if wrong | How to validate |
|---|---|---|
| L_eff = 3 | Largest unknown. L_eff = 2 → ~5 pp savings; L_eff = 5 → ~18 pp | Pull Orca Whirlpool's tick liquidity distribution on-chain; sum weighted by inverse distance to spot. |
| σ = 70% | Quadratic effect on LVR | Compute realized vol from 30d minute bars before quoting externally. |
| Real keeper τ distribution | 30× correction is the squishiest number; real factor could be 10× or 100× | Devnet telemetry — log every keeper update timestamp, build the lag distribution. |
| Fee offset fraction (40%) | Direct effect on CLMM baseline | Solve the M-M-R-T 2023 fees-LVR equation numerically with actual σ and γ instead of using rule-of-thumb. |

## 8. Caveats

- **The 30× real-world correction is engineering judgment, not a measurement.**
  Replace it the moment you have devnet/canary data on actual keeper timing,
  realized adverse selection, and vol regime distribution.
- **L_eff varies with liquidity migration.** A single number is an annual
  average at best; in practice it shifts hour-by-hour as LPs rebalance.
- **The model is silent on directional flow toxicity** (CEX-DEX latency arb
  against on-chain order books). That is *additional* loss for CLMMs and is
  likely the dominant real-world LVR source — meaning the CLMM number here is
  conservative (too low). Net Nomad savings should be *higher* than this
  document estimates, not lower.
- **No published empirical measurement of an oracle AMM's realized LVR** is
  cited because the author is not aware of one. Lifinity has self-reported
  numbers; they are not independently validated. The 30× correction is a
  hedge against model error, not a data-grounded estimate.

## 9. References

- Milionis, J., Moallemi, C. C., Roughgarden, T., Tsoukalas, A. (2022).
  *Automated Market Making and Loss-Versus-Rebalancing.*
  arXiv:2208.06046 — canonical theoretical framework for LVR.
- Milionis, J., Moallemi, C. C., Roughgarden, T., Tsoukalas, A. (2023).
  *Automated Market Making and Arbitrage Profits in the Presence of Fees.*
  arXiv:2305.14604 — adds fee-tier discount to the LVR rate.

## 10. Out-of-scope (next steps)

- On-chain measurement harness: log every swap with `(oracle price, pool quote,
  fill price, payload age, signed flow)`; sum realized adverse selection over
  fixed windows; compare to model prediction.
- Counterfactual backtest: replay historical Pyth Lazer payloads + minute-bar
  SOL prices through the Nomad curve; diff LP P&L against an Orca Whirlpool
  position with matched TVL and tick distribution.
- Compare against on-chain Orca SOL/USDC LP P&L (Dune); subtract fee revenue
  from total LP return; the residual benchmarks the realized LVR drag.
