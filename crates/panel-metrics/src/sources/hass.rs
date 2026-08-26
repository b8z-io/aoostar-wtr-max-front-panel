// SPDX-License-Identifier: MIT OR Apache-2.0

//! Home Assistant entity states.
//!
//! Entities are named explicitly in the config rather than bulk-fetched. Recon found entity
//! IDs are not guessable — `hypervolt_charge_power` did not exist while
//! `hypervolt_session_energy` did — so naming them makes a rename show up as a metric that
//! stopped being emitted, rather than as a row that quietly disappears.

use super::client_for;
use crate::config::Hass;
use crate::metrics::Metric;
use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct State {
    entity_id: String,
    state: String,
    #[serde(default)]
    attributes: Attributes,
}

#[derive(Debug, Default, Deserialize)]
struct Attributes {
    #[serde(default)]
    unit_of_measurement: Option<String>,
}

/// Convert one entity state into a metric.
///
/// Home Assistant reports every state as a string, including the ones that are not numbers
/// at all — `unavailable`, `unknown`, `on`, `off`. Booleans map to 1 and 0 because a panel
/// can render those; everything else non-numeric is dropped, since inventing a number for
/// `unavailable` would be exactly the kind of plausible lie this project keeps refusing to
/// tell.
fn to_metric(state: &State) -> Option<Metric> {
    let value = match state.state.trim().to_ascii_lowercase().as_str() {
        "on" | "home" | "open" | "true" => 1.0,
        "off" | "not_home" | "closed" | "false" => 0.0,
        "unavailable" | "unknown" | "none" | "" => return None,
        other => other.parse::<f64>().ok()?,
    };

    let mut metric = Metric::new("hass_entity_state", value)
        .label("entity", &state.entity_id)
        .help("Home Assistant entity state as a number");

    if let Some(unit) = &state.attributes.unit_of_measurement {
        metric = metric.label("unit", unit);
    }

    Some(metric)
}

pub async fn scrape(cfg: &Hass) -> Result<Vec<Metric>> {
    let token = cfg.endpoint.secret()?;
    let client = client_for(&cfg.endpoint)?;
    let base = cfg.endpoint.url.trim_end_matches('/');
    let mut metrics = Vec::new();

    for entity in &cfg.entities {
        let url = format!("{base}/api/states/{entity}");
        let response = client
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .with_context(|| format!("Home Assistant request for {entity} failed"))?;

        if !response.status().is_success() {
            // One renamed entity should not cost the rest. Its metric simply stops being
            // emitted, which the consumer's staleness layer surfaces on its own.
            log::warn!(
                "Home Assistant entity {entity} returned {}",
                response.status()
            );
            continue;
        }

        match response.json::<State>().await {
            Ok(state) => metrics.extend(to_metric(&state)),
            Err(e) => log::warn!("Could not read Home Assistant entity {entity}: {e:#}"),
        }
    }

    Ok(metrics)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(json: &str) -> State {
        serde_json::from_str(json).unwrap()
    }

    /// From the live response recorded in ops/RECON-sources.md.
    const SPEEDTEST: &str = r#"{"entity_id":"sensor.speedtest_tracker_download",
        "state":"891.0","attributes":{"unit_of_measurement":"Mbit/s",
        "friendly_name":"Speedtest Tracker Download"}}"#;

    #[test]
    fn the_recorded_response_becomes_a_metric() {
        let m = to_metric(&state(SPEEDTEST)).expect("should convert");
        assert_eq!(m.value, 891.0);
        assert_eq!(
            m.labels,
            vec![
                ("entity".into(), "sensor.speedtest_tracker_download".into()),
                ("unit".into(), "Mbit/s".into())
            ]
        );
    }

    #[test]
    fn an_entity_without_a_unit_still_converts() {
        let m = to_metric(&state(r#"{"entity_id":"sensor.x","state":"3136"}"#)).unwrap();
        assert_eq!(m.value, 3136.0);
        assert_eq!(m.labels.len(), 1, "only the entity label");
    }

    #[test]
    fn boolean_states_map_to_one_and_zero() {
        for (raw, expected) in [("on", 1.0), ("off", 0.0), ("home", 1.0), ("not_home", 0.0)] {
            let json = format!(r#"{{"entity_id":"x","state":"{raw}"}}"#);
            assert_eq!(to_metric(&state(&json)).unwrap().value, expected, "{raw}");
        }
    }

    /// Inventing a number for "unavailable" would be the plausible lie this whole project
    /// exists to avoid. Dropping it lets the staleness layer show "--" instead.
    #[test]
    fn unavailable_states_are_dropped_rather_than_guessed() {
        for raw in ["unavailable", "unknown", "none", ""] {
            let json = format!(r#"{{"entity_id":"x","state":"{raw}"}}"#);
            assert!(
                to_metric(&state(&json)).is_none(),
                "{raw} should be dropped"
            );
        }
    }

    #[test]
    fn a_non_numeric_state_is_dropped() {
        let json = r#"{"entity_id":"x","state":"charging"}"#;
        assert!(to_metric(&state(json)).is_none());
    }

    #[test]
    fn negative_and_fractional_values_survive() {
        let json = r#"{"entity_id":"x","state":"-1.5"}"#;
        assert_eq!(to_metric(&state(json)).unwrap().value, -1.5);
    }
}
