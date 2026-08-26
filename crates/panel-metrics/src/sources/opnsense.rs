// SPDX-License-Identifier: MIT OR Apache-2.0

//! OPNsense system and gateway health.
//!
//! Two things shape this source. Its PHP backend answers in 50-200ms and can return 503
//! under concurrent load, so requests are issued one after another rather than joined. And
//! per the recon notes, this integration is API-only: interactive shell commands on that
//! box have caused a full network outage before.

use super::client_for;
use crate::config::OpnSense;
use crate::metrics::Metric;
use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct SystemStatus {
    #[serde(default)]
    metadata: Metadata,
}

#[derive(Debug, Default, Deserialize)]
struct Metadata {
    #[serde(default)]
    system: Option<SystemSection>,
}

#[derive(Debug, Deserialize)]
struct SystemSection {
    #[serde(default)]
    status: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct GatewayStatus {
    #[serde(default)]
    items: Vec<Gateway>,
}

#[derive(Debug, Deserialize)]
struct Gateway {
    name: String,
    #[serde(default)]
    status_translated: Option<String>,
    #[serde(default)]
    loss: Option<String>,
    #[serde(default)]
    delay: Option<String>,
}

/// OPNsense reports "~" where it has no measurement, which is not a number and must not
/// become one. Percentages and milliseconds arrive with their unit attached.
fn parse_measurement(raw: &str) -> Option<f64> {
    let cleaned = raw.trim().trim_end_matches(['%', 's', 'm']).trim();
    if cleaned.is_empty() || cleaned == "~" {
        return None;
    }
    cleaned.parse::<f64>().ok()
}

fn gateway_up(status: &str) -> f64 {
    if status.eq_ignore_ascii_case("online") {
        1.0
    } else {
        0.0
    }
}

fn build(system: Option<SystemStatus>, gateways: Option<GatewayStatus>) -> Vec<Metric> {
    let mut metrics = Vec::new();

    if let Some(status) = system
        .and_then(|s| s.metadata.system)
        .and_then(|s| s.status)
    {
        // 2 is healthy, 1 updates available, 0 pending reboot.
        metrics.push(
            Metric::new("opnsense_system_status", status)
                .help("OPNsense system status: 2 healthy, 1 updates available, 0 pending reboot"),
        );
    }

    if let Some(gateways) = gateways {
        for gateway in &gateways.items {
            let up = gateway
                .status_translated
                .as_deref()
                .map(gateway_up)
                .unwrap_or(0.0);
            metrics.push(
                Metric::new("opnsense_gateway_up", up)
                    .label("gateway", &gateway.name)
                    .help("Whether OPNsense reports this gateway online"),
            );

            if let Some(loss) = gateway.loss.as_deref().and_then(parse_measurement) {
                metrics.push(
                    Metric::new("opnsense_gateway_loss_percent", loss)
                        .label("gateway", &gateway.name)
                        .help("Packet loss to the gateway monitor address"),
                );
            }
            if let Some(delay) = gateway.delay.as_deref().and_then(parse_measurement) {
                metrics.push(
                    Metric::new("opnsense_gateway_delay_ms", delay)
                        .label("gateway", &gateway.name)
                        .help("Round trip delay to the gateway monitor address"),
                );
            }
        }

        metrics.push(
            Metric::new(
                "opnsense_gateways_down",
                gateways
                    .items
                    .iter()
                    .filter(|g| {
                        g.status_translated
                            .as_deref()
                            .map(gateway_up)
                            .unwrap_or(0.0)
                            < 1.0
                    })
                    .count() as f64,
            )
            .help("Gateways not reporting online"),
        );
    }

    metrics
}

pub async fn scrape(cfg: &OpnSense) -> Result<Vec<Metric>> {
    let key = cfg.key()?;
    let secret = cfg.endpoint.secret()?;
    let client = client_for(&cfg.endpoint)?;
    let base = cfg.endpoint.url.trim_end_matches('/');

    let get = async |path: &str| -> Result<reqwest::Response> {
        let response = client
            .get(format!("{base}{path}"))
            .basic_auth(&key, Some(&secret))
            .send()
            .await
            .with_context(|| format!("OPNsense request to {path} failed"))?;
        if !response.status().is_success() {
            anyhow::bail!("OPNsense {path} returned {}", response.status());
        }
        Ok(response)
    };

    // Serialised, never joined: the PHP backend returns 503 under concurrent requests.
    // system/status doubles as the auth probe, so a credential problem surfaces here.
    let system: SystemStatus = get("/api/core/system/status").await?.json().await?;
    let gateways = match get("/api/routes/gateway/status").await {
        Ok(response) => response.json::<GatewayStatus>().await.ok(),
        Err(e) => {
            log::warn!("OPNsense gateway status unavailable: {e:#}");
            None
        }
    };

    Ok(build(Some(system), gateways))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// From the live responses recorded in ops/RECON-sources.md.
    const SYSTEM: &str = r#"{"metadata":{"system":{"status":2,"message":"No pending messages",
        "title":"System"},"subsystems":[]}}"#;

    const GATEWAYS: &str = r#"{"items":[{"name":"WAN_GW","address":"151.226.144.1",
        "status":"none","loss":"~","delay":"~","stddev":"~","monitor":"~",
        "status_translated":"Online"}],"status":"ok"}"#;

    fn parse(system: &str, gateways: &str) -> Vec<Metric> {
        build(
            Some(serde_json::from_str(system).unwrap()),
            Some(serde_json::from_str(gateways).unwrap()),
        )
    }

    fn find(metrics: &[Metric], name: &str) -> Option<f64> {
        metrics.iter().find(|m| m.name == name).map(|m| m.value)
    }

    #[test]
    fn the_recorded_responses_parse() {
        let m = parse(SYSTEM, GATEWAYS);
        assert_eq!(find(&m, "opnsense_system_status"), Some(2.0));
        assert_eq!(find(&m, "opnsense_gateway_up"), Some(1.0));
        assert_eq!(find(&m, "opnsense_gateways_down"), Some(0.0));
    }

    #[test]
    fn the_gateway_is_labelled_by_name() {
        let m = parse(SYSTEM, GATEWAYS);
        let gw = m.iter().find(|m| m.name == "opnsense_gateway_up").unwrap();
        assert_eq!(gw.labels, vec![("gateway".into(), "WAN_GW".into())]);
    }

    /// "~" means OPNsense has no measurement. Turning it into 0 would report a perfect
    /// link with zero loss and zero latency.
    #[test]
    fn a_tilde_measurement_is_omitted_not_zeroed() {
        let m = parse(SYSTEM, GATEWAYS);
        assert_eq!(find(&m, "opnsense_gateway_loss_percent"), None);
        assert_eq!(find(&m, "opnsense_gateway_delay_ms"), None);
    }

    #[test]
    fn real_measurements_are_parsed_with_their_units_stripped() {
        assert_eq!(parse_measurement("0.5%"), Some(0.5));
        assert_eq!(parse_measurement("12.3ms"), Some(12.3));
        assert_eq!(parse_measurement(" 7 "), Some(7.0));
    }

    #[test]
    fn empty_and_tilde_measurements_are_none() {
        assert_eq!(parse_measurement("~"), None);
        assert_eq!(parse_measurement(""), None);
        assert_eq!(parse_measurement("   "), None);
    }

    #[test]
    fn a_measured_gateway_reports_loss_and_delay() {
        let m = parse(
            SYSTEM,
            r#"{"items":[{"name":"WAN_GW","loss":"1.5%","delay":"11.2ms",
                "status_translated":"Online"}]}"#,
        );
        assert_eq!(find(&m, "opnsense_gateway_loss_percent"), Some(1.5));
        assert_eq!(find(&m, "opnsense_gateway_delay_ms"), Some(11.2));
    }

    #[test]
    fn an_offline_gateway_is_counted_down() {
        let m = parse(
            SYSTEM,
            r#"{"items":[{"name":"WAN_GW","status_translated":"Offline"}]}"#,
        );
        assert_eq!(find(&m, "opnsense_gateway_up"), Some(0.0));
        assert_eq!(find(&m, "opnsense_gateways_down"), Some(1.0));
    }

    /// An unknown status word is not proof the gateway is fine.
    #[test]
    fn an_unrecognised_status_is_not_up() {
        assert_eq!(gateway_up("Pending"), 0.0);
        assert_eq!(gateway_up("online"), 1.0, "case should not matter");
    }

    #[test]
    fn system_status_survives_the_gateways_being_unavailable() {
        let m = build(Some(serde_json::from_str(SYSTEM).unwrap()), None);
        assert_eq!(find(&m, "opnsense_system_status"), Some(2.0));
        assert_eq!(find(&m, "opnsense_gateways_down"), None);
    }
}
