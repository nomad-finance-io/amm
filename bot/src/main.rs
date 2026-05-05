use anyhow::Result;
use solana_sdk::signer::Signer;
use std::process::ExitCode;
use tokio::time::MissedTickBehavior;

mod config;
mod keeper;
mod pusher;
mod pyth;
mod state;

use config::Config;
use keeper::TickError;

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();

    match run().await {
        Ok(()) => {
            tracing::info!("clean shutdown");
            ExitCode::SUCCESS
        }
        Err(e) => {
            tracing::error!(error = ?e, "fatal error");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<()> {
    let cfg = Config::from_env()?;
    let signer_pubkey = cfg.keypair.pubkey();

    tracing::info!(
        pool = %cfg.pool,
        feed_id = cfg.feed_id,
        channel = %cfg.channel,
        interval_ms = cfg.interval.as_millis(),
        signer = %signer_pubkey,
        "bot starting"
    );

    // One-shot: validate this signer is the pool's oracle_keeper, that the
    // env feed id matches the pool's, and cache the pool/config fields the
    // quote math needs on every tick.
    let cache = {
        let cfg = cfg.clone();
        tokio::task::spawn_blocking(move || {
            state::load_blocking(&cfg.rpc_url, cfg.pool, signer_pubkey, cfg.feed_id)
        })
        .await??
    };
    tracing::info!(
        min_spread_bps = cache.min_spread_bps,
        base_trade_fee_rate = cache.base_trade_fee_rate,
        "pool validated"
    );

    // Background: Pyth Lazer subscriber feeds latest quote into a watch cell.
    // It never sends a transaction.
    let quote_rx = pyth::spawn(&cfg);

    // Foreground: every INTERVAL_MS, push one update_pool_oracle tx.
    let mut ticker = tokio::time::interval(cfg.interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                match keeper::tick(&cfg, &cache, &quote_rx).await {
                    Ok(sig) => tracing::info!(%sig, "pushed oracle update"),
                    Err(TickError::Transient(msg)) => {
                        tracing::warn!(reason = %msg, "tick skipped; will retry next interval");
                    }
                    Err(TickError::Fatal(msg)) => {
                        tracing::error!(reason = %msg, "fatal program error; exiting");
                        return Err(anyhow::anyhow!(msg));
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("SIGINT received; shutting down");
                return Ok(());
            }
        }
    }
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
}
