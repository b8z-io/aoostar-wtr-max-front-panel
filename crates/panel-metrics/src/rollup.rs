// SPDX-License-Identifier: MIT OR Apache-2.0

//! One number answering "is anything wrong".
//!
//! Every other metric here is a count of things that are fine: 69 monitors up, 5 pools
//! online, 0 gateways down. A panel that enumerates five zeros teaches you to stop reading
//! it, because the healthy state and the state you must act on look almost identical --
//! you have to notice which of five figures changed. Summing the problem counters into a
//! single figure inverts that: the healthy state is one glyph, `0`, and anything else is
//! immediately different from the shape you have learned to expect.
//!
//! The detail metrics are still emitted. This is a headline, not a replacement -- the
//! panel shows the roll-up large and the contributors small.

use crate::metrics::Metric;

/// The problem counters that make up the headline figure.
///
/// Deliberately only counters where a non-zero value means "something needs attention".
/// Load average, uptime and speedtest figures are readings, not faults, and adding faults
/// to readings would produce a number that means nothing.
const CONTRIBUTORS: [&str; 4] = [
    "truenas_pools_unhealthy",
    "kuma_monitors_down",
    "kuma_certificates_invalid",
    "opnsense_gateways_down",
];

/// Sources whose failure makes the roll-up unknowable.
///
/// Home Assistant is absent on purpose: it contributes readings, not faults, so losing it
/// costs a tile rather than invalidating the headline.
const REQUIRED: [&str; 3] = ["truenas", "kuma", "opnsense"];

fn source_is_up(metrics: &[Metric], source: &str) -> bool {
    metrics.iter().any(|m| {
        m.name == "panel_metrics_source_up"
            && m.value == 1.0
            && m.labels.iter().any(|(k, v)| k == "source" && v == source)
    })
}

fn sources_down(metrics: &[Metric]) -> f64 {
    metrics
        .iter()
        .filter(|m| m.name == "panel_metrics_source_up" && m.value != 1.0)
        .count() as f64
}

/// Derive the summary metrics from a completed scrape.
///
/// `homelab_problems_total` is emitted **only** when every required source reported up.
/// A partial sum would be the most dangerous output this service could produce: `0` while
/// blind to a whole source is indistinguishable on the panel from `0` because everything
/// is genuinely fine. Omitting it instead lets the key go stale downstream, and the panel
/// draws `--`, which is the truth.
///
/// A source that is up but did not emit one of its counters contributes 0, which is a
/// different case entirely: Kuma omits `kuma_certificates_invalid` when it monitors no
/// certificates, and "no certificates to be invalid" really is zero problems.
pub fn summarise(metrics: &[Metric]) -> Vec<Metric> {
    let mut out = vec![
        Metric::new("homelab_sources_down", sources_down(metrics))
            .help("Configured sources whose last scrape failed"),
    ];

    if !REQUIRED.iter().all(|s| source_is_up(metrics, s)) {
        return out;
    }

    let total: f64 = CONTRIBUTORS
        .iter()
        .map(|name| {
            metrics
                .iter()
                .find(|m| m.name == *name)
                .map(|m| m.value)
                .unwrap_or(0.0)
        })
        .sum();

    out.push(
        Metric::new("homelab_problems_total", total)
            .help("Unhealthy pools, monitors down, invalid certificates and gateways down"),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn up(source: &str, value: f64) -> Metric {
        Metric::new("panel_metrics_source_up", value).label("source", source)
    }

    fn all_up() -> Vec<Metric> {
        REQUIRED.iter().map(|s| up(s, 1.0)).collect()
    }

    fn value(metrics: &[Metric], name: &str) -> Option<f64> {
        metrics.iter().find(|m| m.name == name).map(|m| m.value)
    }

    #[test]
    fn a_healthy_homelab_sums_to_zero() {
        let mut input = all_up();
        input.push(Metric::new("truenas_pools_unhealthy", 0.0));
        input.push(Metric::new("kuma_monitors_down", 0.0));
        input.push(Metric::new("kuma_certificates_invalid", 0.0));
        input.push(Metric::new("opnsense_gateways_down", 0.0));

        assert_eq!(
            value(&summarise(&input), "homelab_problems_total"),
            Some(0.0)
        );
    }

    #[test]
    fn problems_from_every_source_are_added_together() {
        let mut input = all_up();
        input.push(Metric::new("truenas_pools_unhealthy", 1.0));
        input.push(Metric::new("kuma_monitors_down", 3.0));
        input.push(Metric::new("kuma_certificates_invalid", 2.0));
        input.push(Metric::new("opnsense_gateways_down", 1.0));

        assert_eq!(
            value(&summarise(&input), "homelab_problems_total"),
            Some(7.0)
        );
    }

    /// The property the whole module exists for: never report "0 problems" while blind.
    #[test]
    fn a_dead_source_withholds_the_total_entirely() {
        let mut input = vec![up("truenas", 1.0), up("kuma", 0.0), up("opnsense", 1.0)];
        input.push(Metric::new("truenas_pools_unhealthy", 0.0));
        input.push(Metric::new("opnsense_gateways_down", 0.0));

        let out = summarise(&input);
        assert_eq!(
            value(&out, "homelab_problems_total"),
            None,
            "a partial sum reads as reassurance on the panel"
        );
        assert_eq!(value(&out, "homelab_sources_down"), Some(1.0));
    }

    /// Distinct from the case above: Kuma is alive and simply monitors no certificates.
    #[test]
    fn a_counter_a_live_source_did_not_emit_counts_as_zero() {
        let mut input = all_up();
        input.push(Metric::new("truenas_pools_unhealthy", 0.0));
        input.push(Metric::new("kuma_monitors_down", 1.0));
        input.push(Metric::new("opnsense_gateways_down", 0.0));

        assert_eq!(
            value(&summarise(&input), "homelab_problems_total"),
            Some(1.0)
        );
    }

    /// Home Assistant is a nice-to-have; losing it must not blank the headline figure.
    #[test]
    fn an_optional_source_being_down_does_not_withhold_the_total() {
        let mut input = all_up();
        input.push(up("hass", 0.0));
        input.push(Metric::new("truenas_pools_unhealthy", 0.0));
        input.push(Metric::new("kuma_monitors_down", 0.0));
        input.push(Metric::new("opnsense_gateways_down", 0.0));

        let out = summarise(&input);
        assert_eq!(value(&out, "homelab_problems_total"), Some(0.0));
        assert_eq!(value(&out, "homelab_sources_down"), Some(1.0));
    }

    #[test]
    fn sources_down_is_always_emitted_even_when_the_total_is_not() {
        let out = summarise(&[]);
        assert_eq!(value(&out, "homelab_sources_down"), Some(0.0));
        assert_eq!(value(&out, "homelab_problems_total"), None);
    }
}
