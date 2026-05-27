#!/usr/bin/env python3
"""LVR backtest: replay 7d of Orca SOL/USDC swaps against Binance reference price.

For every swap on the Orca SOL/USDC 0.04% Whirlpool over the past N days:
  1. Compute realized markout LVR for Orca LPs (vs Binance ref price at t+horizon).
  2. Compute counterfactual Nomad LVR for the same flow at oracle quote (ref +/- spread/2).
  3. Report the differential.

Outputs `docs/lvr-backtest-results.md` and `docs/lvr-backtest-daily.csv`.
See README.md for usage.
"""

from __future__ import annotations

import argparse
import asyncio
import bisect
import logging
import os
import sys
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Optional

import aiohttp
import pandas as pd
from dotenv import load_dotenv

# Constants -------------------------------------------------------------------

USDC_MINT = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
SOL_MINT = "So11111111111111111111111111111111111111112"

# Orca SOL/USDC 0.04% Whirlpool. Verify on Solscan before relying on output.
DEFAULT_POOL = "Czfq3xZZDmsdGdUyrNLtRhGc47cXcZtLG4crryfu44zE"
ORCA_FEE_BPS = 4

NOMAD_FULL_SPREAD_BPS_DEFAULT = 5
DEFAULT_MARKOUT_SECONDS = 60
DEFAULT_DAYS = 7
DEFAULT_CONCURRENCY = 20  # parallel POSTs to /v0/transactions; raise on Pro/Business tiers
SIG_PAGE_SIZE = 1000      # max for getSignaturesForAddress
TX_BATCH_SIZE = 100       # max for POST /v0/transactions

SCRIPT_DIR = Path(__file__).resolve().parent
DATA_DIR = SCRIPT_DIR / "data"
REPO_ROOT = SCRIPT_DIR.parent.parent
DOCS_DIR = REPO_ROOT / "docs"

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
log = logging.getLogger("backtest")


# HTTP helpers ----------------------------------------------------------------

async def _get_with_retry(session: aiohttp.ClientSession, url: str, params: dict, retries: int = 6):
    last: Optional[Exception] = None
    for attempt in range(retries):
        try:
            async with session.get(url, params=params, timeout=aiohttp.ClientTimeout(total=60)) as resp:
                if resp.status == 429:
                    wait = min(60, 2 ** attempt)
                    log.warning(f"429 rate limit, sleeping {wait}s")
                    await asyncio.sleep(wait)
                    continue
                if resp.status >= 500:
                    wait = 2 ** attempt
                    log.warning(f"{resp.status} server error, retry in {wait}s")
                    await asyncio.sleep(wait)
                    continue
                resp.raise_for_status()
                return await resp.json()
        except aiohttp.ClientError as e:
            last = e
            wait = 2 ** attempt
            log.warning(f"client error {e!r}, retry in {wait}s")
            await asyncio.sleep(wait)
    raise RuntimeError(f"exhausted retries: {last!r}")


# Helius: Orca swaps ----------------------------------------------------------

async def fetch_orca_swaps(
    session: aiohttp.ClientSession,
    pool: str,
    start_ts: int,
    end_ts: int,
    helius_key: str,
    concurrency: int = DEFAULT_CONCURRENCY,
) -> pd.DataFrame:
    """Pull all parsed swaps on `pool` between [start_ts, end_ts) via Helius.

    Two-stage flow optimized for paid Helius tiers:
      1. getSignaturesForAddress RPC enumerates all sigs in the window (1000/page).
      2. POST /v0/transactions enhances batches of 100 sigs in parallel.

    Cached to parquet so re-runs in the same window are instant.
    """
    cache = DATA_DIR / f"orca_swaps_{pool[:8]}_{start_ts}_{end_ts}.parquet"
    if cache.exists():
        log.info(f"swaps cache hit: {cache.name}")
        return pd.read_parquet(cache)

    sigs = await _enumerate_signatures(session, pool, start_ts, end_ts, helius_key)
    log.info(f"enumerated {len(sigs):,} signatures in window")

    if not sigs:
        df = pd.DataFrame(columns=["timestamp", "signature", "sol_delta_pool", "usdc_delta_pool", "executed_price"])
    else:
        rows = await _batch_enhance_signatures(session, sigs, helius_key, start_ts, end_ts, concurrency)
        if not rows:
            log.warning("no swaps parsed from enhanced txs")
            df = pd.DataFrame(columns=["timestamp", "signature", "sol_delta_pool", "usdc_delta_pool", "executed_price"])
        else:
            df = pd.DataFrame(rows).sort_values("timestamp").reset_index(drop=True)

    DATA_DIR.mkdir(exist_ok=True)
    df.to_parquet(cache)
    log.info(f"cached {len(df):,} swaps to {cache.name}")
    return df


async def _enumerate_signatures(
    session: aiohttp.ClientSession,
    pool: str,
    start_ts: int,
    end_ts: int,
    helius_key: str,
) -> list[str]:
    """Page through getSignaturesForAddress until we cross start_ts."""
    rpc_url = f"https://mainnet.helius-rpc.com/?api-key={helius_key}"
    sigs: list[str] = []
    before: Optional[str] = None
    page_idx = 0
    last_log = time.time()

    while True:
        opts: dict = {"limit": SIG_PAGE_SIZE}
        if before:
            opts["before"] = before
        body = {"jsonrpc": "2.0", "id": 1, "method": "getSignaturesForAddress", "params": [pool, opts]}

        data = await _post_with_retry(session, rpc_url, body)
        result = data.get("result") or []
        if not result:
            break

        for s in result:
            bt = s.get("blockTime")
            sig = s.get("signature")
            if bt is None or sig is None:
                continue
            if bt > end_ts:
                continue
            if bt < start_ts:
                continue
            sigs.append(sig)

        oldest_bt = result[-1].get("blockTime") or 0
        before = result[-1]["signature"]
        page_idx += 1

        if time.time() - last_log > 5:
            log.info(
                f"sig page {page_idx}, oldest={datetime.fromtimestamp(oldest_bt, tz=timezone.utc):%Y-%m-%d %H:%M}, "
                f"in-window={len(sigs):,}"
            )
            last_log = time.time()

        if oldest_bt and oldest_bt < start_ts:
            log.info(f"sig enumeration done at page {page_idx}")
            break

    return sigs


async def _batch_enhance_signatures(
    session: aiohttp.ClientSession,
    sigs: list[str],
    helius_key: str,
    start_ts: int,
    end_ts: int,
    concurrency: int,
) -> list[dict]:
    """POST sigs in batches of 100 to /v0/transactions, fan out with semaphore."""
    url = f"https://api.helius.xyz/v0/transactions?api-key={helius_key}"
    sem = asyncio.Semaphore(concurrency)
    batches = [sigs[i:i + TX_BATCH_SIZE] for i in range(0, len(sigs), TX_BATCH_SIZE)]
    log.info(f"enhancing {len(batches):,} batches of up to {TX_BATCH_SIZE}, concurrency={concurrency}")

    async def fetch_batch(batch: list[str]) -> list[dict]:
        async with sem:
            data = await _post_with_retry(session, url, {"transactions": batch})
            if not isinstance(data, list):
                return []
            return data

    rows: list[dict] = []
    completed = 0
    last_log = time.time()
    tasks = [asyncio.create_task(fetch_batch(b)) for b in batches]
    for coro in asyncio.as_completed(tasks):
        txs = await coro
        for tx in txs:
            ts = int(tx.get("timestamp", 0))
            if ts < start_ts or ts > end_ts:
                continue
            parsed = parse_swap_tx(tx)
            if parsed:
                rows.append(parsed)
        completed += 1
        if time.time() - last_log > 5:
            log.info(f"  enhance progress: {completed}/{len(batches)} batches, {len(rows):,} swaps parsed")
            last_log = time.time()

    log.info(f"enhance done: {completed}/{len(batches)} batches, {len(rows):,} swaps parsed")
    return rows


async def _post_with_retry(session: aiohttp.ClientSession, url: str, body: dict, retries: int = 6):
    last: Optional[Exception] = None
    for attempt in range(retries):
        try:
            async with session.post(url, json=body, timeout=aiohttp.ClientTimeout(total=60)) as resp:
                if resp.status == 429:
                    wait = min(60, 2 ** attempt)
                    log.warning(f"429 rate limit, sleeping {wait}s")
                    await asyncio.sleep(wait)
                    continue
                if resp.status >= 500:
                    wait = 2 ** attempt
                    log.warning(f"{resp.status} server error, retry in {wait}s")
                    await asyncio.sleep(wait)
                    continue
                resp.raise_for_status()
                return await resp.json()
        except aiohttp.ClientError as e:
            last = e
            wait = 2 ** attempt
            log.warning(f"client error {e!r}, retry in {wait}s")
            await asyncio.sleep(wait)
    raise RuntimeError(f"exhausted retries: {last!r}")


def parse_swap_tx(tx: dict) -> Optional[dict]:
    """Extract pool's net (SOL, USDC) delta from a Helius parsed swap event.

    Returns None if the tx isn't a clean SOL<->USDC swap.
    """
    swap = (tx.get("events") or {}).get("swap")
    if not swap:
        return None

    sol_to_pool = 0.0
    usdc_to_pool = 0.0
    sol_from_pool = 0.0
    usdc_from_pool = 0.0

    def amt(t: dict) -> float:
        rta = t.get("rawTokenAmount") or {}
        try:
            return float(rta.get("tokenAmount", "0")) / (10 ** int(rta.get("decimals", 0)))
        except (ValueError, TypeError):
            return 0.0

    for ti in (swap.get("tokenInputs") or []):
        m = ti.get("mint")
        a = amt(ti)
        if m == SOL_MINT:
            sol_to_pool += a
        elif m == USDC_MINT:
            usdc_to_pool += a

    for to_ in (swap.get("tokenOutputs") or []):
        m = to_.get("mint")
        a = amt(to_)
        if m == SOL_MINT:
            sol_from_pool += a
        elif m == USDC_MINT:
            usdc_from_pool += a

    # Native SOL legs (unwrapped). Helius returns lamports as a number.
    for native, sink in ((swap.get("nativeInput"), "in"), (swap.get("nativeOutput"), "out")):
        if not native:
            continue
        try:
            lamports = int(native.get("amount") or 0)
        except (ValueError, TypeError):
            continue
        sol_amt = lamports / 1e9
        if sink == "in":
            sol_to_pool += sol_amt
        else:
            sol_from_pool += sol_amt

    sol_delta_pool = sol_to_pool - sol_from_pool
    usdc_delta_pool = usdc_to_pool - usdc_from_pool

    if abs(sol_delta_pool) < 1e-6 or abs(usdc_delta_pool) < 1e-3:
        return None
    if sol_delta_pool * usdc_delta_pool > 0:
        return None  # both legs same sign = not a normal swap

    executed_price = abs(usdc_delta_pool) / abs(sol_delta_pool)
    if executed_price < 1 or executed_price > 100_000:
        return None  # implausible -> likely parsing miss on a multi-hop

    return {
        "timestamp": int(tx["timestamp"]),
        "signature": tx["signature"],
        "sol_delta_pool": sol_delta_pool,
        "usdc_delta_pool": usdc_delta_pool,
        "executed_price": executed_price,
    }


# Binance: reference price ----------------------------------------------------

async def fetch_binance_klines(
    session: aiohttp.ClientSession,
    symbol: str,
    interval: str,
    start_ts: int,
    end_ts: int,
) -> pd.DataFrame:
    """1-minute klines for `symbol`. Returns timestamp (unix s, bar OPEN) + close."""
    cache = DATA_DIR / f"binance_{symbol}_{interval}_{start_ts}_{end_ts}.parquet"
    if cache.exists():
        log.info(f"klines cache hit: {cache.name}")
        return pd.read_parquet(cache)

    url = "https://api.binance.com/api/v3/klines"
    interval_ms = {"1s": 1_000, "1m": 60_000, "5m": 300_000}[interval]
    rows: list[dict] = []
    cursor = start_ts * 1000

    while cursor < end_ts * 1000:
        params = {
            "symbol": symbol,
            "interval": interval,
            "startTime": cursor,
            "endTime": end_ts * 1000,
            "limit": 1000,
        }
        bars = await _get_with_retry(session, url, params)
        if not bars:
            break
        for b in bars:
            rows.append({"timestamp": int(b[0] // 1000), "close": float(b[4])})
        last_open = int(bars[-1][0])
        next_cursor = last_open + interval_ms
        if next_cursor <= cursor:
            break
        cursor = next_cursor

    df = pd.DataFrame(rows).drop_duplicates("timestamp").sort_values("timestamp").reset_index(drop=True)
    DATA_DIR.mkdir(exist_ok=True)
    df.to_parquet(cache)
    log.info(f"cached {len(df)} {symbol} {interval} bars to {cache.name}")
    return df


# LVR computation -------------------------------------------------------------

@dataclass
class BacktestResult:
    n_swaps: int
    total_volume_usd: float
    orca_markout_pnl: float       # signed: LP markout P&L from order flow
    orca_fee_revenue: float       # always positive
    orca_net_pnl: float           # markout + fee
    nomad_markout_pnl: float      # spread already in quote, so this IS net
    savings_usd: float            # nomad_net - orca_net
    savings_pct: float            # savings as % of |orca_net|


def _lookup_price(ts_arr, close_arr, ts: int) -> Optional[float]:
    """Closest binance bar at or before ts. None if out of range."""
    idx = bisect.bisect_right(ts_arr, ts) - 1
    if idx < 0 or idx >= len(ts_arr):
        return None
    return float(close_arr[idx])


def compute_lvr(
    swaps: pd.DataFrame,
    prices: pd.DataFrame,
    markout_seconds: int,
    nomad_full_spread_bps: float,
    orca_fee_bps: float,
    daily_csv_path: Path,
) -> BacktestResult:
    """Per-swap markout LVR for Orca actual + Nomad counterfactual.

    Sign convention: pool_sol_delta > 0 means pool gained SOL (trader sold).
    LP markout P&L = pool_sol_delta * (ref_price_after - executed_price).
    Negative = LP got adversely selected.
    """
    if swaps.empty or prices.empty:
        return BacktestResult(0, 0, 0, 0, 0, 0, 0, 0)

    ts_arr = prices["timestamp"].values
    close_arr = prices["close"].values

    half_spread = (nomad_full_spread_bps / 10_000) / 2
    rows = []

    for s in swaps.itertuples(index=False):
        t = int(s.timestamp)
        ref_now = _lookup_price(ts_arr, close_arr, t)
        ref_after = _lookup_price(ts_arr, close_arr, t + markout_seconds)
        if ref_now is None or ref_after is None:
            continue

        sol_delta = float(s.sol_delta_pool)
        executed = float(s.executed_price)

        # Drop rows whose executed price is implausibly far from ref (router/MEV noise).
        if abs(executed - ref_now) / ref_now > 0.05:
            continue

        orca_markout = sol_delta * (ref_after - executed)
        notional_usd = abs(float(s.usdc_delta_pool))
        orca_fee = notional_usd * (orca_fee_bps / 10_000)

        if sol_delta > 0:
            nomad_quote = ref_now * (1 - half_spread)
        else:
            nomad_quote = ref_now * (1 + half_spread)
        nomad_markout = sol_delta * (ref_after - nomad_quote)

        rows.append({
            "timestamp": t,
            "notional_usd": notional_usd,
            "orca_markout": orca_markout,
            "orca_fee": orca_fee,
            "nomad_markout": nomad_markout,
        })

    if not rows:
        return BacktestResult(0, 0, 0, 0, 0, 0, 0, 0)

    df = pd.DataFrame(rows)
    total_vol = float(df["notional_usd"].sum())
    orca_mk = float(df["orca_markout"].sum())
    orca_fee = float(df["orca_fee"].sum())
    orca_net = orca_mk + orca_fee
    nomad_mk = float(df["nomad_markout"].sum())
    savings = nomad_mk - orca_net
    savings_pct = (savings / abs(orca_net) * 100) if orca_net != 0 else 0.0

    df["date"] = pd.to_datetime(df["timestamp"], unit="s", utc=True).dt.date
    daily = df.groupby("date").agg(
        n_swaps=("notional_usd", "count"),
        volume_usd=("notional_usd", "sum"),
        orca_markout=("orca_markout", "sum"),
        orca_fee=("orca_fee", "sum"),
        nomad_markout=("nomad_markout", "sum"),
    ).reset_index()
    daily["orca_net"] = daily["orca_markout"] + daily["orca_fee"]
    daily["savings"] = daily["nomad_markout"] - daily["orca_net"]
    daily.to_csv(daily_csv_path, index=False)
    log.info(f"per-day breakdown -> {daily_csv_path}")

    return BacktestResult(
        n_swaps=len(df),
        total_volume_usd=total_vol,
        orca_markout_pnl=orca_mk,
        orca_fee_revenue=orca_fee,
        orca_net_pnl=orca_net,
        nomad_markout_pnl=nomad_mk,
        savings_usd=savings,
        savings_pct=savings_pct,
    )


# Report ----------------------------------------------------------------------

def write_report(
    result: BacktestResult,
    pool: str,
    start_ts: int,
    end_ts: int,
    markout_seconds: int,
    nomad_spread_bps: float,
    orca_fee_bps: float,
    out_path: Path,
) -> None:
    days = (end_ts - start_ts) / 86400
    start = datetime.fromtimestamp(start_ts, tz=timezone.utc)
    end = datetime.fromtimestamp(end_ts, tz=timezone.utc)
    annualized_savings = (result.savings_usd / days) * 365.25 if days > 0 else 0

    report = f"""# LVR Backtest — Nomad Oracle AMM vs Orca Whirlpool (SOL/USDC)

**Status:** Empirical backtest against historical on-chain swaps.
**Last updated:** {datetime.now(timezone.utc):%Y-%m-%d}

## TL;DR

Over **{days:.1f} days** of real Orca SOL/USDC 0.04% Whirlpool flow
({result.n_swaps:,} swaps, ${result.total_volume_usd/1e6:.1f}M total volume), an
oracle-anchored AMM with {nomad_spread_bps} bps full spread would have netted LPs
**${result.savings_usd:,.0f}** more than Orca did over the same window — a
**{result.savings_pct:.1f}%** improvement in LP P&L from order flow.

Annualized: **${annualized_savings:,.0f}/yr** of LP savings on a pool of this size.

## Method

For each historical swap on `{pool}`:

1. **Orca actual** — record the on-chain executed price.
2. **Reference price** — Binance SOLUSDC 1-minute close at the swap timestamp,
   and again at *t* + {markout_seconds}s (markout horizon).
3. **Orca markout P&L** = `pool_sol_delta × (ref_after − executed_price)`
   (signed: negative = LP got adversely selected).
4. **Orca fee revenue** = `{orca_fee_bps} bps × notional`.
5. **Nomad quote** = `ref_now × (1 ± {nomad_spread_bps/2:.1f} bps)`, side chosen by trade direction.
6. **Nomad markout P&L** = `pool_sol_delta × (ref_after − nomad_quote)` —
   spread is already inside the quote, so this is the LP's *net* P&L.

LP savings = `nomad_net − orca_net`.

## Results

| Metric | Orca (actual) | Nomad (counterfactual) |
|---|---:|---:|
| Swaps replayed | {result.n_swaps:,} | {result.n_swaps:,} |
| Total volume | ${result.total_volume_usd/1e6:,.2f}M | ${result.total_volume_usd/1e6:,.2f}M |
| Markout P&L | ${result.orca_markout_pnl:,.0f} | ${result.nomad_markout_pnl:,.0f} |
| Fee revenue | ${result.orca_fee_revenue:,.0f} | (in spread) |
| **Net LP P&L** | **${result.orca_net_pnl:,.0f}** | **${result.nomad_markout_pnl:,.0f}** |

| | Value |
|---|---:|
| LP savings ({days:.1f}d) | **${result.savings_usd:,.0f}** |
| LP savings (annualized) | **${annualized_savings:,.0f}/yr** |
| Improvement vs Orca | **{result.savings_pct:.1f}%** |

Per-day breakdown: `docs/lvr-backtest-daily.csv`.

## Window

- Start: {start:%Y-%m-%d %H:%M} UTC
- End:   {end:%Y-%m-%d %H:%M} UTC
- Pool:  `{pool}` (Orca SOL/USDC, {orca_fee_bps} bps tier)
- Reference: Binance SOLUSDC 1-minute close, {markout_seconds}s markout horizon
- Nomad spread: {nomad_spread_bps} bps full (matches `min_spread_bps` config)

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
- **Markout horizon.** {markout_seconds}s is the standard CEX-DEX adverse-selection
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
"""
    out_path.write_text(report)
    log.info(f"wrote report to {out_path}")


# Main ------------------------------------------------------------------------

async def main_async(args):
    load_dotenv(REPO_ROOT / ".env")
    helius_key = os.environ.get("HELIUS_API_KEY")
    if not helius_key:
        log.error("HELIUS_API_KEY not in env. Set it in .env at the repo root.")
        sys.exit(1)

    # Last N full UTC days, ending at today's UTC midnight. Stable cache key.
    today_midnight = datetime.now(tz=timezone.utc).replace(hour=0, minute=0, second=0, microsecond=0)
    end_ts = int(today_midnight.timestamp())
    start_ts = end_ts - args.days * 86400

    log.info(f"window: {datetime.fromtimestamp(start_ts, tz=timezone.utc)} -> {datetime.fromtimestamp(end_ts, tz=timezone.utc)}")
    log.info(f"pool:   {args.pool} (verify is SOL/USDC 0.04% on Solscan)")
    log.info(f"spread: {args.spread_bps} bps  markout: {args.markout}s  concurrency: {args.concurrency}")

    DATA_DIR.mkdir(exist_ok=True)
    DOCS_DIR.mkdir(exist_ok=True)

    conn = aiohttp.TCPConnector(limit=args.concurrency * 2)
    async with aiohttp.ClientSession(connector=conn) as session:
        swaps_task = fetch_orca_swaps(session, args.pool, start_ts, end_ts, helius_key, args.concurrency)
        prices_task = fetch_binance_klines(session, "SOLUSDC", "1m", start_ts, end_ts + 120)
        swaps, prices = await asyncio.gather(swaps_task, prices_task)

    log.info(f"swaps={len(swaps):,}, ref bars={len(prices):,}")

    daily_csv = DOCS_DIR / "lvr-backtest-daily.csv"
    result = compute_lvr(swaps, prices, args.markout, args.spread_bps, ORCA_FEE_BPS, daily_csv)

    report_path = DOCS_DIR / "lvr-backtest-results.md"
    write_report(result, args.pool, start_ts, end_ts, args.markout, args.spread_bps, ORCA_FEE_BPS, report_path)

    print()
    print("=" * 60)
    print(f"Swaps replayed:   {result.n_swaps:,}")
    print(f"Volume:           ${result.total_volume_usd/1e6:,.2f}M")
    print(f"Orca net LP P&L:  ${result.orca_net_pnl:,.0f}")
    print(f"Nomad net LP P&L: ${result.nomad_markout_pnl:,.0f}")
    print(f"Savings:          ${result.savings_usd:,.0f}  ({result.savings_pct:.1f}%)")
    print(f"Report:           {report_path}")
    print("=" * 60)


def main():
    p = argparse.ArgumentParser(description="LVR backtest: Nomad oracle AMM vs Orca CLMM (SOL/USDC).")
    p.add_argument("--days", type=int, default=DEFAULT_DAYS, help=f"Lookback window in full UTC days (default: {DEFAULT_DAYS})")
    p.add_argument("--pool", default=DEFAULT_POOL, help="Orca Whirlpool address (default: SOL/USDC 0.04%% tier)")
    p.add_argument("--markout", type=int, default=DEFAULT_MARKOUT_SECONDS, help=f"Markout horizon seconds (default: {DEFAULT_MARKOUT_SECONDS})")
    p.add_argument("--spread-bps", type=float, default=NOMAD_FULL_SPREAD_BPS_DEFAULT, dest="spread_bps", help=f"Nomad full spread bps (default: {NOMAD_FULL_SPREAD_BPS_DEFAULT})")
    p.add_argument("--concurrency", type=int, default=DEFAULT_CONCURRENCY, help=f"Parallel POST batches to /v0/transactions (default: {DEFAULT_CONCURRENCY}). Raise for paid Helius tiers (Pro: 50, Business: 100+).")
    args = p.parse_args()
    asyncio.run(main_async(args))


if __name__ == "__main__":
    main()
