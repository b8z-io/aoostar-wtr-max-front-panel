// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: Copyright (c) 2025 Markus Zehnder

//! Timestamped sensor value store with staleness resolution.
//!
//! Sensor values written by external providers carry no indication of age. Without one, a
//! provider that dies leaves its last values in place and they render as though live —
//! which is most misleading precisely when something is wrong.
//!
//! Every value stored here is stamped with the time it was written and the source file it
//! came from. [`SensorStore::resolve`] treats a value older than its source's configured
//! maximum age as absent.
//!
//! Staleness is opt-in: with no maximum age configured, nothing is ever stale and behaviour
//! is identical to a plain value map.

use log::info;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Handle for the sensor file a value was read from. Index into [`SensorStore::sources`].
pub type SourceId = usize;

/// Prefix for internally computed source-health sensors.
pub const SYS_PREFIX: &str = "SYS_";

/// A sensor value together with its provenance.
#[derive(Debug, Clone)]
pub struct SensorValue {
    pub value: String,
    pub updated: Instant,
    pub source: SourceId,
}

/// Whether a resolved value is current or has aged out.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ValueState {
    Live,
    Stale,
}

/// Maximum acceptable age for sensor values, globally and per source.
#[derive(Debug, Default, Clone)]
pub struct StalenessConfig {
    default_max_age: Option<Duration>,
    per_source: HashMap<String, Duration>,
}

impl StalenessConfig {
    pub fn new(default_max_age: Option<Duration>) -> Self {
        Self {
            default_max_age,
            per_source: HashMap::new(),
        }
    }

    /// Set a per-source override, keyed by sensor file stem (`host.txt` → `host`).
    pub fn set_source_max_age(&mut self, source: impl Into<String>, max_age: Duration) {
        self.per_source.insert(source.into(), max_age);
    }

    /// Staleness handling is active only once some maximum age is configured.
    pub fn enabled(&self) -> bool {
        self.default_max_age.is_some() || !self.per_source.is_empty()
    }

    /// Resolution order: per-source override, then global default, then never stale.
    pub fn max_age_for(&self, source: &str) -> Option<Duration> {
        self.per_source
            .get(source)
            .copied()
            .or(self.default_max_age)
    }
}

/// Sensor values keyed by label, with age tracking per source.
#[derive(Debug, Default)]
pub struct SensorStore {
    values: HashMap<String, SensorValue>,
    /// Source file stems, indexed by [`SourceId`].
    sources: Vec<String>,
    staleness: StalenessConfig,
}

impl SensorStore {
    pub fn new(staleness: StalenessConfig) -> Self {
        Self {
            values: HashMap::new(),
            sources: Vec::new(),
            staleness: staleness.clone(),
        }
    }

    pub fn staleness_enabled(&self) -> bool {
        self.staleness.enabled()
    }

    /// Intern a source name, returning its stable handle.
    pub fn source_id(&mut self, name: &str) -> SourceId {
        if let Some(idx) = self.sources.iter().position(|s| s == name) {
            return idx;
        }
        self.sources.push(name.to_string());
        self.sources.len() - 1
    }

    pub fn source_name(&self, id: SourceId) -> Option<&str> {
        self.sources.get(id).map(String::as_str)
    }

    pub fn insert(&mut self, key: String, value: String, source: SourceId, now: Instant) {
        self.values.insert(
            key,
            SensorValue {
                value,
                updated: now,
                source,
            },
        );
    }

    /// Resolve a sensor label to its current value.
    ///
    /// Returns `None` when the label is unknown, or when its value is older than the
    /// maximum age configured for its source.
    pub fn resolve(&self, label: &str, now: Instant) -> Option<&str> {
        let entry = self.values.get(label)?;
        if self.is_expired(entry, now) {
            return None;
        }
        Some(entry.value.as_str())
    }

    fn is_expired(&self, entry: &SensorValue, now: Instant) -> bool {
        let Some(source) = self.sources.get(entry.source) else {
            return false;
        };
        let Some(max_age) = self.staleness.max_age_for(source) else {
            return false;
        };
        now.saturating_duration_since(entry.updated) > max_age
    }

    /// Time since the most recent write from the given source.
    pub fn source_age(&self, source: SourceId, now: Instant) -> Option<Duration> {
        self.values
            .values()
            .filter(|v| v.source == source)
            .map(|v| v.updated)
            .max()
            .map(|newest| now.saturating_duration_since(newest))
    }

    /// A source is live when its newest value is within its configured maximum age.
    pub fn source_live(&self, source: SourceId, now: Instant) -> bool {
        let Some(age) = self.source_age(source, now) else {
            return false;
        };
        let Some(name) = self.sources.get(source) else {
            return false;
        };
        match self.staleness.max_age_for(name) {
            Some(max_age) => age <= max_age,
            None => true,
        }
    }

    pub fn sources_total(&self) -> usize {
        self.sources.len()
    }

    pub fn sources_live(&self, now: Instant) -> usize {
        (0..self.sources.len())
            .filter(|id| self.source_live(*id, now))
            .count()
    }

    /// Resolve an internally computed `SYS_` source-health sensor.
    ///
    /// These are calculated at render time rather than stored, so they are never stale
    /// themselves and remain meaningful when every provider is dead.
    ///
    /// | Label | Value |
    /// |---|---|
    /// | `SYS_sources_total` | number of known sensor sources |
    /// | `SYS_sources_live` | number currently within their maximum age |
    /// | `SYS_sources_health` | live/total as a percentage |
    /// | `SYS_source_<stem>_age` | seconds since that source last updated |
    /// | `SYS_source_<stem>_live` | `1` or `0` |
    pub fn sys_value(&self, label: &str, now: Instant) -> Option<String> {
        let rest = label.strip_prefix(SYS_PREFIX)?;

        match rest {
            "sources_total" => return Some(self.sources_total().to_string()),
            "sources_live" => return Some(self.sources_live(now).to_string()),
            "sources_health" => {
                let health = (self.sources_live(now) * 100)
                    .checked_div(self.sources_total())
                    .unwrap_or(0);
                return Some(health.to_string());
            }
            _ => {}
        }

        let rest = rest.strip_prefix("source_")?;
        // Source stems may contain underscores, so match the suffix rather than splitting.
        for (suffix, is_age) in [("_age", true), ("_live", false)] {
            let Some(stem) = rest.strip_suffix(suffix) else {
                continue;
            };
            let Some(id) = self.sources.iter().position(|s| s == stem) else {
                continue;
            };
            return Some(if is_age {
                self.source_age(id, now)
                    .map(|d| d.as_secs().to_string())
                    .unwrap_or_else(|| "0".to_string())
            } else {
                u8::from(self.source_live(id, now)).to_string()
            });
        }

        None
    }

    pub fn log_summary(&self, now: Instant) {
        if !self.staleness_enabled() {
            return;
        }
        info!(
            "Sensor sources: {}/{} live",
            self.sources_live(now),
            self.sources_total()
        );
    }

    #[cfg(test)]
    pub fn insert_at(&mut self, key: &str, value: &str, source: &str, updated: Instant) {
        let id = self.source_id(source);
        self.insert(key.to_string(), value.to_string(), id, updated);
    }
}

/// Parse a `--max-age-file`: `<sensor file stem>: <seconds>` per line.
pub fn parse_max_age_entries(
    entries: &HashMap<String, String>,
    cfg: &mut StalenessConfig,
) -> anyhow::Result<()> {
    for (source, seconds) in entries {
        let secs: u64 = seconds.trim().parse().map_err(|e| {
            anyhow::anyhow!("Invalid max age '{seconds}' for source '{source}': {e}")
        })?;
        cfg.set_source_max_age(source.clone(), Duration::from_secs(secs));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn store_with_default(secs: u64) -> SensorStore {
        SensorStore::new(StalenessConfig::new(Some(Duration::from_secs(secs))))
    }

    #[test]
    fn fresh_value_resolves() {
        let now = Instant::now();
        let mut store = store_with_default(10);
        store.insert_at("cpu", "42", "host", now);

        assert_eq!(store.resolve("cpu", now), Some("42"));
    }

    #[test]
    fn value_past_max_age_resolves_to_none() {
        let now = Instant::now();
        let mut store = store_with_default(10);
        store.insert_at("cpu", "42", "host", now - Duration::from_secs(11));

        assert_eq!(store.resolve("cpu", now), None);
    }

    #[test]
    fn value_exactly_at_max_age_is_still_live() {
        let now = Instant::now();
        let mut store = store_with_default(10);
        store.insert_at("cpu", "42", "host", now - Duration::from_secs(10));

        assert_eq!(store.resolve("cpu", now), Some("42"));
    }

    #[test]
    fn per_source_override_wins_over_default() {
        let now = Instant::now();
        let mut cfg = StalenessConfig::new(Some(Duration::from_secs(10)));
        cfg.set_source_max_age("homelab", Duration::from_secs(120));
        let mut store = SensorStore::new(cfg);

        store.insert_at("cpu", "42", "host", now - Duration::from_secs(30));
        store.insert_at("pool", "ok", "homelab", now - Duration::from_secs(30));

        assert_eq!(store.resolve("cpu", now), None, "host uses the 10s default");
        assert_eq!(
            store.resolve("pool", now),
            Some("ok"),
            "homelab uses its 120s override"
        );
    }

    /// Regression guard for the opt-in requirement: unconfigured means never stale.
    #[test]
    fn nothing_is_stale_without_configuration() {
        let now = Instant::now();
        let mut store = SensorStore::new(StalenessConfig::default());
        store.insert_at("cpu", "42", "host", now - Duration::from_secs(86_400));

        assert!(!store.staleness_enabled());
        assert_eq!(store.resolve("cpu", now), Some("42"));
    }

    #[test]
    fn unknown_label_resolves_to_none() {
        let now = Instant::now();
        let store = store_with_default(10);

        assert_eq!(store.resolve("nope", now), None);
    }

    #[test]
    fn source_ids_are_interned() {
        let mut store = store_with_default(10);
        let a = store.source_id("host");
        let b = store.source_id("host");
        let c = store.source_id("homelab");

        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(store.sources_total(), 2);
    }

    #[test]
    fn sources_live_counts_a_mix() {
        let now = Instant::now();
        let mut store = store_with_default(10);
        store.insert_at("a", "1", "host", now);
        store.insert_at("b", "2", "homelab", now - Duration::from_secs(60));

        assert_eq!(store.sources_total(), 2);
        assert_eq!(store.sources_live(now), 1);
    }

    #[test]
    fn newest_value_determines_source_liveness() {
        let now = Instant::now();
        let mut store = store_with_default(10);
        store.insert_at("old", "1", "host", now - Duration::from_secs(60));
        store.insert_at("new", "2", "host", now);

        assert!(
            store.source_live(0, now),
            "a source is live if anything from it is current"
        );
    }

    #[rstest]
    #[case("SYS_sources_total", "2")]
    #[case("SYS_sources_live", "1")]
    #[case("SYS_sources_health", "50")]
    #[case("SYS_source_host_live", "1")]
    #[case("SYS_source_homelab_live", "0")]
    #[case("SYS_source_homelab_age", "60")]
    fn sys_sensors_report_source_health(#[case] label: &str, #[case] expected: &str) {
        let now = Instant::now();
        let mut store = store_with_default(10);
        store.insert_at("a", "1", "host", now);
        store.insert_at("b", "2", "homelab", now - Duration::from_secs(60));

        assert_eq!(store.sys_value(label, now).as_deref(), Some(expected));
    }

    #[test]
    fn sys_health_is_zero_when_all_sources_are_stale() {
        let now = Instant::now();
        let mut store = store_with_default(10);
        store.insert_at("a", "1", "host", now - Duration::from_secs(60));
        store.insert_at("b", "2", "homelab", now - Duration::from_secs(60));

        assert_eq!(
            store.sys_value("SYS_sources_health", now).as_deref(),
            Some("0")
        );
    }

    #[test]
    fn sys_health_is_hundred_when_all_sources_are_fresh() {
        let now = Instant::now();
        let mut store = store_with_default(10);
        store.insert_at("a", "1", "host", now);
        store.insert_at("b", "2", "homelab", now);

        assert_eq!(
            store.sys_value("SYS_sources_health", now).as_deref(),
            Some("100")
        );
    }

    #[test]
    fn source_stems_containing_underscores_are_matched() {
        let now = Instant::now();
        let mut store = store_with_default(10);
        store.insert_at("a", "1", "my_host_metrics", now);

        assert_eq!(
            store
                .sys_value("SYS_source_my_host_metrics_live", now)
                .as_deref(),
            Some("1")
        );
    }

    #[test]
    fn non_sys_labels_are_not_claimed() {
        let now = Instant::now();
        let store = store_with_default(10);

        assert_eq!(store.sys_value("cpu_temperature", now), None);
        assert_eq!(store.sys_value("SYS_nonsense", now), None);
    }

    #[test]
    fn max_age_entries_parse_into_config() {
        let mut entries = HashMap::new();
        entries.insert("host".to_string(), "10".to_string());
        entries.insert("homelab".to_string(), " 90 ".to_string());
        let mut cfg = StalenessConfig::default();

        parse_max_age_entries(&entries, &mut cfg).expect("should parse");

        assert_eq!(cfg.max_age_for("host"), Some(Duration::from_secs(10)));
        assert_eq!(cfg.max_age_for("homelab"), Some(Duration::from_secs(90)));
        assert!(cfg.enabled());
    }

    #[test]
    fn invalid_max_age_entry_is_an_error() {
        let mut entries = HashMap::new();
        entries.insert("host".to_string(), "soon".to_string());
        let mut cfg = StalenessConfig::default();

        assert!(parse_max_age_entries(&entries, &mut cfg).is_err());
    }
}
