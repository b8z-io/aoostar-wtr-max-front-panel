// SPDX-License-Identifier: MIT OR Apache-2.0

//! TrueNAS pool health and capacity.
//!
//! HTTPS only — port 80 does not answer. The API is synchronous and blocks during a ZFS
//! scrub or resilver, which is exactly when you most want to look at the panel, so the
//! per-source timeout matters here more than anywhere else.

use super::client_for;
use crate::config::TrueNas;
use crate::metrics::Metric;
use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct SystemInfo {
    uptime_seconds: Option<f64>,
    #[serde(default)]
    loadavg: Vec<f64>,
}

#[derive(Debug, Deserialize)]
struct Pool {
    name: String,
    status: String,
    #[serde(default)]
    scan: Option<Scan>,
}

#[derive(Debug, Deserialize)]
struct Scan {
    #[serde(default)]
    errors: Option<f64>,
    #[serde(default)]
    percentage: Option<f64>,
    #[serde(default)]
    state: Option<String>,
}

/// Turn a pool's status string into a number a panel can colour.
///
/// Anything other than ONLINE is a problem worth seeing, and the distinction between
/// DEGRADED and FAULTED matters less on a small display than the fact that it is not
/// healthy — but they are kept separate so a threshold can tell "act soon" from "act now".
fn pool_health(status: &str) -> f64 {
    match status.to_ascii_uppercase().as_str() {
        "ONLINE" => 1.0,
        "DEGRADED" => 0.5,
        _ => 0.0,
    }
}

fn build(info: Option<SystemInfo>, pools: Vec<Pool>) -> Vec<Metric> {
    let mut metrics = Vec::new();

    if let Some(info) = info {
        if let Some(uptime) = info.uptime_seconds {
            metrics.push(
                Metric::new("truenas_uptime_seconds", uptime).help("TrueNAS uptime in seconds"),
            );
            // Seconds are the truth; days are what a panel can render. 450203 is six
            // glyphs of noise at a glance, 5.2 is a fact. The division happens here
            // because nothing downstream of this service performs arithmetic.
            metrics.push(
                Metric::new("truenas_uptime_days", uptime / 86_400.0)
                    .help("TrueNAS uptime in days"),
            );
        }
        if let Some(load) = info.loadavg.first() {
            metrics.push(Metric::new("truenas_load1", *load).help("TrueNAS 1 minute load average"));
        }
    }

    for pool in &pools {
        metrics.push(
            Metric::new("truenas_pool_health", pool_health(&pool.status))
                .label("pool", &pool.name)
                .help("Pool health: 1 online, 0.5 degraded, 0 otherwise"),
        );

        if let Some(scan) = &pool.scan {
            if let Some(errors) = scan.errors {
                metrics.push(
                    Metric::new("truenas_pool_scan_errors", errors)
                        .label("pool", &pool.name)
                        .help("Errors found by the last scrub or resilver"),
                );
            }
            // Only meaningful while a scan is running; a finished scan sits at 100 and
            // would otherwise look like permanent activity.
            let running = scan
                .state
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case("SCANNING"));
            if running && let Some(pct) = scan.percentage {
                metrics.push(
                    Metric::new("truenas_pool_scan_percent", pct)
                        .label("pool", &pool.name)
                        .help("Progress of a scrub or resilver in progress"),
                );
            }
        }
    }

    metrics.push(
        Metric::new("truenas_pools_total", pools.len() as f64).help("Pools known to TrueNAS"),
    );
    // Emitted as well as the unhealthy count, because the panel renders "5 / 5" and cannot
    // subtract. Arithmetic belongs here; downstream there is nothing that performs any.
    metrics.push(
        Metric::new(
            "truenas_pools_online",
            pools
                .iter()
                .filter(|p| pool_health(&p.status) >= 1.0)
                .count() as f64,
        )
        .help("Pools reporting ONLINE"),
    );
    metrics.push(
        Metric::new(
            "truenas_pools_unhealthy",
            pools
                .iter()
                .filter(|p| pool_health(&p.status) < 1.0)
                .count() as f64,
        )
        .help("Pools not reporting ONLINE"),
    );

    metrics
}

pub async fn scrape(cfg: &TrueNas) -> Result<Vec<Metric>> {
    let token = cfg.endpoint.secret()?;
    let client = client_for(&cfg.endpoint)?;
    let base = cfg.endpoint.url.trim_end_matches('/');

    let get = async |path: &str| -> Result<reqwest::Response> {
        let url = format!("{base}{path}");
        let response = client
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .with_context(|| format!("TrueNAS request to {path} failed"))?;
        if !response.status().is_success() {
            anyhow::bail!("TrueNAS {path} returned {}", response.status());
        }
        Ok(response)
    };

    // Pools are the point of this source; system info is a bonus. Failing the whole scrape
    // because a secondary endpoint was slow would lose the pool health too.
    let pools: Vec<Pool> = get("/api/v2.0/pool").await?.json().await?;
    let info = match get("/api/v2.0/system/info").await {
        Ok(response) => response.json::<SystemInfo>().await.ok(),
        Err(e) => {
            log::warn!("TrueNAS system info unavailable: {e:#}");
            None
        }
    };

    Ok(build(info, pools))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Taken from the live response recorded in ops/RECON-sources.md.
    const POOLS: &str = r#"[{"id":1,"name":"vault","status":"ONLINE","path":"/mnt/vault",
        "scan":{"function":"SCRUB","state":"FINISHED","errors":0,"percentage":100.0}}]"#;

    const INFO: &str = r#"{"version":"25.04.2.6","hostname":"truenas","cores":4,
        "loadavg":[0.25,0.1,0.0],"uptime_seconds":375754,"ecc_memory":true}"#;

    fn parse(pools: &str, info: &str) -> Vec<Metric> {
        build(
            Some(serde_json::from_str(info).unwrap()),
            serde_json::from_str(pools).unwrap(),
        )
    }

    fn value(metrics: &[Metric], name: &str) -> f64 {
        metrics
            .iter()
            .find(|m| m.name == name)
            .unwrap_or_else(|| panic!("{name} should be emitted"))
            .value
    }

    #[test]
    fn the_recorded_response_parses() {
        let m = parse(POOLS, INFO);
        assert_eq!(value(&m, "truenas_pool_health"), 1.0);
        assert_eq!(value(&m, "truenas_pools_total"), 1.0);
        assert_eq!(value(&m, "truenas_pools_unhealthy"), 0.0);
        assert_eq!(value(&m, "truenas_uptime_seconds"), 375754.0);
        assert_eq!(value(&m, "truenas_load1"), 0.25);
    }

    #[test]
    fn the_pool_is_labelled_by_name() {
        let m = parse(POOLS, INFO);
        let pool = m.iter().find(|m| m.name == "truenas_pool_health").unwrap();
        assert_eq!(pool.labels, vec![("pool".into(), "vault".into())]);
    }

    #[test]
    fn health_maps_status_to_a_number() {
        assert_eq!(pool_health("ONLINE"), 1.0);
        assert_eq!(pool_health("online"), 1.0, "case should not matter");
        assert_eq!(pool_health("DEGRADED"), 0.5);
        assert_eq!(pool_health("FAULTED"), 0.0);
        assert_eq!(pool_health("UNKNOWN"), 0.0, "unrecognised is not healthy");
    }

    #[test]
    fn a_degraded_pool_counts_as_unhealthy() {
        let m = parse(
            r#"[{"id":1,"name":"vault","status":"DEGRADED","path":"/mnt/vault"}]"#,
            INFO,
        );
        assert_eq!(value(&m, "truenas_pools_unhealthy"), 1.0);
        assert_eq!(value(&m, "truenas_pool_health"), 0.5);
    }

    /// A finished scan sits at 100%, which would read as a scrub permanently in progress.
    #[test]
    fn scan_progress_is_only_emitted_while_scanning() {
        let finished = parse(POOLS, INFO);
        assert!(
            !finished
                .iter()
                .any(|m| m.name == "truenas_pool_scan_percent")
        );

        let running = parse(
            r#"[{"id":1,"name":"vault","status":"ONLINE",
                "scan":{"state":"SCANNING","errors":0,"percentage":42.5}}]"#,
            INFO,
        );
        assert_eq!(value(&running, "truenas_pool_scan_percent"), 42.5);
    }

    #[test]
    fn scan_errors_are_reported() {
        let m = parse(
            r#"[{"id":1,"name":"vault","status":"ONLINE",
                "scan":{"state":"FINISHED","errors":3,"percentage":100.0}}]"#,
            INFO,
        );
        assert_eq!(value(&m, "truenas_pool_scan_errors"), 3.0);
    }

    #[test]
    fn a_pool_without_scan_data_still_reports_health() {
        let m = parse(r#"[{"id":1,"name":"vault","status":"ONLINE"}]"#, INFO);
        assert_eq!(value(&m, "truenas_pool_health"), 1.0);
    }

    #[test]
    fn multiple_pools_are_each_reported() {
        let m = parse(
            r#"[{"id":1,"name":"vault","status":"ONLINE"},
                {"id":2,"name":"tank","status":"FAULTED"}]"#,
            INFO,
        );
        assert_eq!(value(&m, "truenas_pools_total"), 2.0);
        assert_eq!(value(&m, "truenas_pools_unhealthy"), 1.0);
        assert_eq!(
            m.iter().filter(|m| m.name == "truenas_pool_health").count(),
            2
        );
    }

    /// Pool health is the point of this source; losing system info must not lose it.
    #[test]
    fn pools_are_still_reported_without_system_info() {
        let m = build(None, serde_json::from_str(POOLS).unwrap());
        assert_eq!(value(&m, "truenas_pool_health"), 1.0);
        assert!(!m.iter().any(|m| m.name == "truenas_uptime_seconds"));
    }
}
