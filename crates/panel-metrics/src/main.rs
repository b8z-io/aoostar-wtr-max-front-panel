// SPDX-License-Identifier: MIT OR Apache-2.0

//! Aggregates homelab APIs into Prometheus text for the AOOSTAR front panel.
//!
//! Sits on docker2 and is scraped by `aster-prom` on pve-nas, which writes a sensor file
//! that `asterctl` renders. This is also where arithmetic lives — counts and sums such as
//! "68 of 72 monitors up" — because nothing further down the chain performs any.
//!
//! # Why it polls on a timer rather than on request
//!
//! Sources are scraped on a schedule and the result is cached. Scraping on request would
//! let the consumer's poll rate drive load on four upstream APIs, and OPNsense in
//! particular returns 503 under concurrent requests.
//!
//! # What happens when a source fails
//!
//! Its metrics are omitted entirely rather than served from the last good scrape, and
//! `panel_metrics_source_up` goes to 0. Serving stale values here would be invisible
//! downstream: the sensor file would still be fresh, so the panel's staleness layer — which
//! works by noticing that keys stop being refreshed — would have nothing to notice.

mod config;
mod metrics;
mod sources;

use anyhow::{Context, Result};
use axum::Router;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use clap::Parser;
use config::Config;
use env_logger::Env;
use log::{error, info};
use metrics::Metric;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// Aggregates homelab APIs into Prometheus text for the AOOSTAR front panel.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Configuration file. Credentials are referenced by path from it, never contained in it.
    #[arg(short, long, default_value = "/etc/panel-metrics/config.toml")]
    config: PathBuf,

    /// Scrape every configured source once, print the result, and exit.
    ///
    /// For checking credentials and connectivity during deployment without leaving a
    /// service running.
    #[arg(long)]
    once: bool,
}

/// The most recent successful render, served to every caller until the next scrape.
type Snapshot = Arc<RwLock<String>>;

async fn scrape_all(config: &Config) -> Vec<Metric> {
    let mut metrics = Vec::new();

    if let Some(cfg) = &config.truenas {
        metrics.extend(sources::collect(
            "truenas",
            sources::truenas::scrape(cfg).await,
        ));
    }
    if let Some(cfg) = &config.kuma {
        metrics.extend(sources::collect("kuma", sources::kuma::scrape(cfg).await));
    }
    if let Some(cfg) = &config.opnsense {
        metrics.extend(sources::collect(
            "opnsense",
            sources::opnsense::scrape(cfg).await,
        ));
    }
    if let Some(cfg) = &config.hass {
        metrics.extend(sources::collect("hass", sources::hass::scrape(cfg).await));
    }

    metrics
}

async fn serve_metrics(State(snapshot): State<Snapshot>) -> impl IntoResponse {
    let body = snapshot
        .read()
        .map(|s| s.clone())
        .unwrap_or_else(|e| e.into_inner().clone());

    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
        )],
        body,
    )
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();
    let args = Args::parse();

    let config = Arc::new(Config::load(&args.config)?);

    if args.once {
        let rendered = metrics::render(&scrape_all(&config).await);
        print!("{rendered}");
        return Ok(());
    }

    // Scrape once before binding, so the first request never sees an empty body and a
    // credential mistake fails at startup rather than silently on a schedule.
    let snapshot: Snapshot = Arc::new(RwLock::new(metrics::render(&scrape_all(&config).await)));

    let scraper = {
        let config = Arc::clone(&config);
        let snapshot = Arc::clone(&snapshot);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(config.refresh());
            ticker.tick().await; // the first tick is immediate; we already scraped
            loop {
                ticker.tick().await;
                let rendered = metrics::render(&scrape_all(&config).await);
                match snapshot.write() {
                    Ok(mut guard) => *guard = rendered,
                    Err(e) => *e.into_inner() = rendered,
                }
            }
        })
    };

    let app = Router::new()
        .route("/metrics", get(serve_metrics))
        .with_state(Arc::clone(&snapshot));

    let listener = tokio::net::TcpListener::bind(&config.listen)
        .await
        .with_context(|| format!("Failed to bind {}", config.listen))?;
    info!(
        "Serving /metrics on {}, refreshing every {}s",
        config.listen,
        config.refresh().as_secs()
    );

    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            info!("Shutting down");
        })
        .await;

    scraper.abort();
    if let Err(e) = result {
        error!("Server stopped: {e}");
    }

    Ok(())
}
