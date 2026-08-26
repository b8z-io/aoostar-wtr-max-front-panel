// SPDX-License-Identifier: MIT OR Apache-2.0

//! Source scraping and the rules for what happens when a source fails.

pub mod hass;
pub mod kuma;
pub mod opnsense;
pub mod truenas;

use crate::config::Endpoint;
use crate::metrics::Metric;
use anyhow::Result;
use log::warn;
use reqwest::Client;
use std::time::Duration;

/// Build a client for one source.
///
/// Per-source rather than shared, because `accept_invalid_cert` and the timeout differ:
/// trusting an appliance's self-signed certificate must not extend that trust to the other
/// three.
pub fn client_for(endpoint: &Endpoint) -> Result<Client> {
    Ok(Client::builder()
        .timeout(endpoint.timeout())
        .connect_timeout(Duration::from_secs(5))
        .danger_accept_invalid_certs(endpoint.accept_invalid_cert)
        .build()?)
}

/// Collect one source's metrics, converting failure into an explicit down signal.
///
/// A failed source contributes **no metrics at all** rather than its last known values.
/// That is the whole point: a stale reading served as though fresh is indistinguishable
/// from a live one downstream, and the consumer's staleness layer only works because keys
/// stop being refreshed. Serving the previous values here would defeat it entirely.
pub fn collect(source: &str, result: Result<Vec<Metric>>) -> Vec<Metric> {
    let up = Metric::new("panel_metrics_source_up", 0.0)
        .label("source", source)
        .help("Whether the last scrape of this source succeeded");

    match result {
        Ok(mut metrics) => {
            metrics.push(Metric { value: 1.0, ..up });
            metrics
        }
        Err(e) => {
            warn!("Scrape of {source} failed: {e:#}");
            vec![up]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_successful_scrape_is_marked_up() {
        let out = collect("truenas", Ok(vec![Metric::new("a", 1.0)]));
        let up = out
            .iter()
            .find(|m| m.name == "panel_metrics_source_up")
            .expect("up metric");

        assert_eq!(up.value, 1.0);
        assert_eq!(up.labels, vec![("source".into(), "truenas".into())]);
        assert_eq!(out.len(), 2, "the source's own metric survives");
    }

    /// The critical property: a failure must not carry the previous values forward.
    #[test]
    fn a_failed_scrape_contributes_only_the_down_signal() {
        let out = collect("truenas", Err(anyhow::anyhow!("connection refused")));

        assert_eq!(out.len(), 1, "no metrics may be served for a dead source");
        assert_eq!(out[0].name, "panel_metrics_source_up");
        assert_eq!(out[0].value, 0.0);
    }
}
