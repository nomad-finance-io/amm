# LVR Backtest

Replays N days of real Orca SOL/USDC swaps and computes:
1. Realized markout LVR for Orca LPs (vs Binance reference price)
2. Counterfactual markout LVR for the same flow on Nomad's oracle AMM
3. The differential, $ saved and % improvement

Outputs `docs/lvr-backtest-results.md` and `docs/lvr-backtest-daily.csv`.

## Setup

Requires Python 3.10+. From this directory:

```bash
python3 -m venv .venv
source .venv/bin/activate
pip install -r requirements.txt
```

Add your Helius key to the repo root `.env`:

```
HELIUS_API_KEY=your_key_here
```

## Run

Defaults: 7 days, Orca SOL/USDC 0.04% pool, 60s markout, 5 bps Nomad spread.

```bash
python backtest.py
```

Override:

```bash
python backtest.py --days 14 --markout 120 --spread-bps 10
python backtest.py --pool <other_whirlpool>
python backtest.py --concurrency 50          # paid Helius Pro tier
python backtest.py --concurrency 100         # paid Helius Business+
```

## Fetch architecture

Two-stage flow optimized for paid Helius tiers:

1. **Enumerate signatures** via `getSignaturesForAddress` RPC (1000/page, sequential).
2. **Enhance in parallel** via `POST /v0/transactions` in batches of 100,
   fanned out with a semaphore set by `--concurrency`.

This is much faster than the address-based enhanced-tx endpoint (which forces
sequential pagination at 100/page).

## What to expect

| Tier | Suggested `--concurrency` | Runtime (7d) |
|---|---:|---:|
| Free / Developer | 5 | ~30-60 min |
| Builder | 20 (default) | ~5-10 min |
| Pro | 50 | ~2-5 min |
| Business+ | 100 | ~1-3 min |

- Re-runs in the same UTC day are instant (cached parquet in `data/`).
- **First-run sanity check:** the script logs the pool address before fetching.
  Verify it on Solscan (search the address) — should be Orca SOL/USDC,
  tickSpacing matching 0.04% tier — before letting the long fetch run.
- If you hit `429`s, the script auto-backs off; if it happens often, lower
  `--concurrency`.

## Method

Per-swap markout, signed by pool inventory delta:

- `LP_pnl_per_swap = pool_sol_delta × (ref_price[t + horizon] − executed_price)`
- Negative = LP got adversely selected.
- Sum across all swaps = realized LVR (before fees).
- Add Orca fee revenue (4 bps × notional) to get net Orca LP P&L.

For Nomad counterfactual: same `pool_sol_delta`, but `executed_price` is
replaced by `ref_now × (1 ± spread/2)`. Spread is already inside the quote, so
no separate fee is added.

## Caveats

See the `Caveats` section of the generated report. Headline assumptions:

- Same flow lands on Nomad (in reality some flow would route elsewhere if
  Nomad's quote is worse).
- Nomad's oracle is fresh at the trade timestamp (real keeper lag adds back
  some LVR on the Nomad side).
- Spread-only Nomad model — ignores virtual-reserve slippage and dynamic
  widening, so the backtest is conservative on Nomad's side.

## Cache layout

```
data/
├── orca_swaps_<pool8>_<start>_<end>.parquet
└── binance_SOLUSDC_1m_<start>_<end>.parquet
```

Delete to force re-fetch.
