//! `update_pool_oracle` instruction sender.
//!
//! `anchor-client 0.32` is sync; the build+send is wrapped in
//! `tokio::task::spawn_blocking` so we don't stall the runtime on RPC I/O.

use anchor_client::{Client, Cluster};
use solana_sdk::compute_budget::ComputeBudgetInstruction;
use solana_sdk::signature::Signature;
use solana_sdk::signer::Signer;

use crate::config::Config;

#[derive(Debug, thiserror::Error)]
pub enum PushError {
    /// On-chain rejected our payload with a code that means "the values you
    /// pushed are wrong" — retrying will not help. Operator must investigate.
    #[error("fatal program error: {0}")]
    Fatal(String),
    /// Transient (RPC, blockhash, network). Worth trying again next tick.
    #[error("transient error: {0}")]
    Transient(String),
}

pub async fn send(
    cfg: &Config,
    effective_bid: i64,
    effective_ask: i64,
    price_exponent: i16,
    dynamic_fee_rate: u64,
) -> Result<Signature, PushError> {
    let cfg = cfg.clone();
    let result = tokio::task::spawn_blocking(move || send_blocking(
        &cfg,
        effective_bid,
        effective_ask,
        price_exponent,
        dynamic_fee_rate,
    ))
    .await
    .map_err(|e| PushError::Transient(format!("spawn_blocking join error: {e}")))?;

    result
}

fn send_blocking(
    cfg: &Config,
    effective_bid: i64,
    effective_ask: i64,
    price_exponent: i16,
    dynamic_fee_rate: u64,
) -> Result<Signature, PushError> {
    // Anchor's `Cluster::Custom` wants both http + ws urls. The bot doesn't
    // currently use a websocket for RPC, so feed the same url for both.
    let cluster = Cluster::Custom(cfg.rpc_url.clone(), cfg.rpc_url.clone());
    let client = Client::new(cluster, cfg.keypair.clone());
    let program = client
        .program(cfg.program_id)
        .map_err(|e| PushError::Transient(format!("program init: {e:?}")))?;

    let result = program
        .request()
        .instruction(ComputeBudgetInstruction::set_compute_unit_price(
            cfg.compute_unit_price_micro_lamports,
        ))
        .instruction(ComputeBudgetInstruction::set_compute_unit_limit(
            cfg.compute_unit_limit,
        ))
        .accounts(nomad_amm::accounts::UpdatePoolOracle {
            oracle_keeper: cfg.keypair.pubkey(),
            pool_state: cfg.pool,
        })
        .args(nomad_amm::instruction::UpdatePoolOracle {
            effective_bid_mantissa: effective_bid,
            effective_ask_mantissa: effective_ask,
            price_exponent,
            dynamic_fee_rate,
        })
        .send();

    result.map_err(classify_error)
}

fn classify_error(err: anchor_client::ClientError) -> PushError {
    let s = format!("{err:?}");
    // Anchor surfaces the on-chain error variant name in its debug output.
    // Match on the names defined in `programs/cp-swap/src/error.rs`.
    const FATAL_CODES: &[&str] = &[
        "InvalidEffectiveSpread",
        "BidNotLessThanAsk",
        "InvalidDynamicFeeRate",
        "InvalidOracleKeeper",
    ];
    if FATAL_CODES.iter().any(|name| s.contains(name)) {
        PushError::Fatal(s)
    } else {
        PushError::Transient(s)
    }
}

