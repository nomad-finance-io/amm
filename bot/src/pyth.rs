//! Pyth Lazer WebSocket subscriber.
//!
//! Spawns a background task that maintains the WS connection (auto-reconnect
//! and dedup are handled by `pyth-lazer-client`) and writes the most recent
//! parsed price update into a `tokio::sync::watch` cell. The interval loop
//! reads the cell on its tick; messages arriving between ticks are silently
//! overwritten — that's the whole point.
//!
//! This task **never sends a transaction**.

use anyhow::{anyhow, bail, Context, Result};
use pyth_lazer_client::stream_client::PythLazerStreamClientBuilder;
use pyth_lazer_client::ws_connection::AnyResponse;
use pyth_lazer_protocol::api::{
    Channel, DeliveryFormat, Format, JsonBinaryEncoding, SubscribeRequest, SubscriptionId,
    SubscriptionParams, SubscriptionParamsRepr, WsResponse,
};
use pyth_lazer_protocol::time::FixedRate;
use pyth_lazer_protocol::{PriceFeedId, PriceFeedProperty};
use tokio::sync::watch;

use crate::config::Config;

#[derive(Debug, Clone, Copy)]
pub struct LatestQuote {
    pub price_mantissa: i64,
    pub confidence_mantissa: Option<i64>,
    pub best_bid_mantissa: Option<i64>,
    pub best_ask_mantissa: Option<i64>,
    pub exponent: i16,
}

/// Spawn the Pyth Lazer subscriber. Returns a watch receiver that yields the
/// latest parsed quote (or `None` until the first message arrives).
pub fn spawn(cfg: &Config) -> watch::Receiver<Option<LatestQuote>> {
    let (tx, rx) = watch::channel::<Option<LatestQuote>>(None);
    let cfg = cfg.clone();
    tokio::spawn(async move {
        if let Err(e) = run(cfg, tx).await {
            tracing::error!(error = ?e, "pyth subscriber exited");
        }
    });
    rx
}

async fn run(cfg: Config, tx: watch::Sender<Option<LatestQuote>>) -> Result<()> {
    let channel = parse_channel(&cfg.channel)?;
    let feed_id = PriceFeedId(cfg.feed_id);

    let mut client = PythLazerStreamClientBuilder::new(cfg.lazer_token.clone())
        .build()
        .context("failed to build Pyth Lazer stream client")?;

    let mut stream = client.start().await.context("failed to start Pyth Lazer stream")?;

    let request = SubscribeRequest {
        subscription_id: SubscriptionId(1),
        params: SubscriptionParams::new(SubscriptionParamsRepr {
            price_feed_ids: Some(vec![feed_id]),
            symbols: None,
            properties: vec![
                PriceFeedProperty::Price,
                PriceFeedProperty::Exponent,
                PriceFeedProperty::Confidence,
                PriceFeedProperty::BestBidPrice,
                PriceFeedProperty::BestAskPrice,
            ],
            formats: vec![Format::Solana],
            delivery_format: DeliveryFormat::Json,
            json_binary_encoding: JsonBinaryEncoding::Base64,
            parsed: true,
            channel,
            ignore_invalid_feeds: false,
        })
        .map_err(|e| anyhow!("invalid subscription params: {e:?}"))?,
    };

    client
        .subscribe(request)
        .await
        .context("Pyth Lazer subscribe call failed")?;

    tracing::info!(feed_id = cfg.feed_id, channel = %cfg.channel, "Pyth Lazer subscribed");

    while let Some(msg) = stream.recv().await {
        if let AnyResponse::Json(WsResponse::StreamUpdated(update)) = msg {
            let Some(parsed) = update.payload.parsed else { continue };

            for feed in parsed.price_feeds {
                if feed.price_feed_id != feed_id {
                    continue;
                }

                let quote = match build_quote(&feed) {
                    Some(q) => q,
                    None => {
                        tracing::debug!("Lazer feed update missing required fields; skipping");
                        continue;
                    }
                };

                if tx.send(Some(quote)).is_err() {
                    // Receiver dropped — main loop is shutting down.
                    return Ok(());
                }
            }
        }
    }

    bail!("Pyth Lazer stream closed unexpectedly")
}

fn build_quote(feed: &pyth_lazer_protocol::api::ParsedFeedPayload) -> Option<LatestQuote> {
    let price = feed.price?.mantissa_i64();
    let exponent = feed.exponent?;
    Some(LatestQuote {
        price_mantissa: price,
        confidence_mantissa: feed.confidence.map(|p| p.mantissa_i64()),
        best_bid_mantissa: feed.best_bid_price.map(|p| p.mantissa_i64()),
        best_ask_mantissa: feed.best_ask_price.map(|p| p.mantissa_i64()),
        exponent,
    })
}

fn parse_channel(raw: &str) -> Result<Channel> {
    match raw {
        "real_time" => Ok(Channel::RealTime),
        "fixed_rate@50ms" => Ok(Channel::FixedRate(FixedRate::RATE_50_MS)),
        "fixed_rate@200ms" => Ok(Channel::FixedRate(FixedRate::RATE_200_MS)),
        "fixed_rate@1000ms" | "fixed_rate@1s" => Ok(Channel::FixedRate(FixedRate::RATE_1000_MS)),
        other => bail!(
            "PYTH_CHANNEL '{other}' unsupported; use 'real_time', 'fixed_rate@50ms', 'fixed_rate@200ms', or 'fixed_rate@1000ms'"
        ),
    }
}
