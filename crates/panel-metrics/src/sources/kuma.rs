// SPDX-License-Identifier: MIT OR Apache-2.0

//! Uptime-Kuma aggregates.
//!
//! Kuma exposes one `uptime_kuma_status` sample per monitor, seventy-odd of them. A panel
//! wants "68 of 72 up", which is a sum — and nothing between Kuma and the display performs
//! arithmetic. `aster-prom` copies values verbatim and `asterctl` renders whatever it is
//! handed. This is where the counting happens.

use super::client_for;
use crate::config::Kuma;
use crate::metrics::Metric;
use anyhow::{Context, Result};

/// One parsed sample from the Prometheus text format.
#[derive(Debug, PartialEq)]
struct Sample<'a> {
    name: &'a str,
    labels: Vec<(&'a str, String)>,
    value: f64,
}

/// Parse Prometheus text into samples.
///
/// Only enough of the format to read gauges with labels: comments and blank lines are
/// skipped, and a trailing millisecond timestamp — which Kuma emits — is ignored.
fn parse(text: &str) -> Vec<Sample<'_>> {
    let mut samples = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let (head, rest) = match line.find('{') {
            Some(open) => {
                let Some(close) = line[open..].find('}') else {
                    continue;
                };
                let close = open + close;
                (&line[..open], (&line[open + 1..close], &line[close + 1..]))
            }
            None => match line.split_once(char::is_whitespace) {
                Some((name, tail)) => (name, ("", tail)),
                None => continue,
            },
        };
        let (label_text, tail) = rest;

        // Kuma appends a millisecond timestamp after the value; take only the first field.
        let Some(value) = tail
            .split_whitespace()
            .next()
            .and_then(|v| v.parse::<f64>().ok())
        else {
            continue;
        };

        samples.push(Sample {
            name: head,
            labels: parse_labels(label_text),
            value,
        });
    }

    samples
}

/// Split a label block into pairs, unescaping quoted values.
fn parse_labels(text: &str) -> Vec<(&str, String)> {
    let mut labels = Vec::new();
    let mut rest = text;

    while let Some(eq) = rest.find('=') {
        let key = rest[..eq].trim_start_matches(',').trim();
        let after = &rest[eq + 1..];
        if !after.starts_with('"') {
            break;
        }

        // Walk the value so an escaped quote does not end it early.
        let mut value = String::new();
        let mut escaped = false;
        let mut end = None;
        for (i, c) in after[1..].char_indices() {
            if escaped {
                value.push(c);
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                end = Some(i + 1);
                break;
            } else {
                value.push(c);
            }
        }
        let Some(end) = end else { break };

        labels.push((key, value));
        rest = &after[end + 1..];
    }

    labels
}

/// Turn Kuma's per-monitor samples into the counts a panel can display.
///
/// Returns `None` when the response contains nothing recognisable as Kuma metrics. That is
/// a failure, not a result: reporting "0 monitors, 0 up, 0 down" from an unparseable
/// response is a confident lie, and it renders on the panel as a plausible three zeros
/// rather than as the absence of data it actually is.
fn aggregate(text: &str) -> Option<Vec<Metric>> {
    let samples = parse(text);
    if !samples.iter().any(|s| s.name.starts_with("uptime_kuma")) {
        return None;
    }

    let statuses: Vec<&Sample> = samples
        .iter()
        .filter(|s| s.name == "uptime_kuma_monitor_status" || s.name == "uptime_kuma_status")
        .collect();

    let total = statuses.len() as f64;
    // Kuma's status is 1 up, 0 down, and also uses 2 for pending and 3 for maintenance.
    // Only 1 counts as up; anything else is explicitly not up, which is the honest reading.
    let up = statuses.iter().filter(|s| s.value == 1.0).count() as f64;

    let mut metrics = vec![
        Metric::new("kuma_monitors_total", total).help("Monitors known to Uptime-Kuma"),
        Metric::new("kuma_monitors_up", up).help("Monitors currently reporting up"),
        Metric::new("kuma_monitors_down", total - up).help("Monitors not currently up"),
    ];

    // A certificate count is only meaningful where certificates exist, so it is emitted
    // only when Kuma reports any.
    let certs: Vec<&Sample> = samples
        .iter()
        .filter(|s| s.name == "uptime_kuma_certificate_valid")
        .collect();
    if !certs.is_empty() {
        let expiring = certs.iter().filter(|s| s.value != 1.0).count() as f64;
        metrics.push(
            Metric::new("kuma_certificates_invalid", expiring)
                .help("Monitored certificates Kuma reports as not valid"),
        );
    }

    Some(metrics)
}

pub async fn scrape(cfg: &Kuma) -> Result<Vec<Metric>> {
    let secret = cfg.endpoint.secret()?;
    let response = client_for(&cfg.endpoint)?
        .get(&cfg.endpoint.url)
        // Kuma expects an empty username with the API key as the password.
        .basic_auth("", Some(secret))
        .send()
        .await
        .context("Uptime-Kuma request failed")?;

    if !response.status().is_success() {
        anyhow::bail!("Uptime-Kuma returned {}", response.status());
    }

    let text = response.text().await?;
    aggregate(&text).with_context(|| {
        // Kuma answers 200 with its dashboard HTML on any path that is not /metrics, so a
        // wrong URL looks like a healthy scrape returning nothing. Say so plainly, and
        // include enough of the body to tell HTML from an empty response.
        let excerpt: String = text.chars().take(60).collect();
        format!(
            "Response contained no uptime_kuma metrics ({} bytes, starts: {excerpt:?}). \
             Check the URL ends in /metrics",
            text.len()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped after the live sample recorded in ops/RECON-sources.md, including the
    /// trailing millisecond timestamps Kuma emits.
    const SAMPLE: &str = r#"
# HELP uptime_kuma_monitor_status Monitor Status (1 = UP, 0 = DOWN)
# TYPE uptime_kuma_monitor_status gauge
uptime_kuma_monitor_status{monitor_name="internet",monitor_url="https://1.1.1.1"} 1 1690387200000
uptime_kuma_monitor_status{monitor_name="opnsense",monitor_url="https://192.168.68.1"} 1 1690387200000
uptime_kuma_monitor_status{monitor_name="traefik",monitor_url="https://t.example"} 0 1690387200000
uptime_kuma_monitor_status{monitor_name="pending one",monitor_url="https://p.example"} 2 1690387200000
# HELP uptime_kuma_certificate_valid Is the certificate valid?
# TYPE uptime_kuma_certificate_valid gauge
uptime_kuma_certificate_valid{monitor_name="traefik"} 1 1690387200000
uptime_kuma_certificate_valid{monitor_name="mail"} 0 1690387200000
"#;

    fn agg(text: &str) -> Vec<Metric> {
        aggregate(text).expect("should recognise Kuma metrics")
    }

    fn value(metrics: &[Metric], name: &str) -> f64 {
        metrics
            .iter()
            .find(|m| m.name == name)
            .unwrap_or_else(|| panic!("{name} should be emitted"))
            .value
    }

    #[test]
    fn monitors_are_counted() {
        let m = agg(SAMPLE);
        assert_eq!(value(&m, "kuma_monitors_total"), 4.0);
        assert_eq!(value(&m, "kuma_monitors_up"), 2.0);
        assert_eq!(value(&m, "kuma_monitors_down"), 2.0);
    }

    /// Kuma uses 2 for pending and 3 for maintenance. Counting anything non-1 as up would
    /// report a broken service as healthy.
    #[test]
    fn only_status_one_counts_as_up() {
        let m = agg(r#"uptime_kuma_monitor_status{monitor_name="a"} 2"#);
        assert_eq!(value(&m, "kuma_monitors_up"), 0.0);
        assert_eq!(value(&m, "kuma_monitors_down"), 1.0);
    }

    #[test]
    fn up_and_down_always_sum_to_total() {
        let m = agg(SAMPLE);
        assert_eq!(
            value(&m, "kuma_monitors_up") + value(&m, "kuma_monitors_down"),
            value(&m, "kuma_monitors_total")
        );
    }

    #[test]
    fn invalid_certificates_are_counted() {
        assert_eq!(value(&agg(SAMPLE), "kuma_certificates_invalid"), 1.0);
    }

    #[test]
    fn the_certificate_metric_is_omitted_when_kuma_reports_none() {
        let m = agg(r#"uptime_kuma_monitor_status{monitor_name="a"} 1"#);
        assert!(!m.iter().any(|m| m.name == "kuma_certificates_invalid"));
    }

    /// Reporting "0 monitors, 0 up" from a response that is not Kuma metrics at all would
    /// render as three plausible zeros on the panel. It has to fail instead.
    #[test]
    fn an_empty_response_is_a_failure_not_a_count_of_zero() {
        assert!(aggregate("").is_none());
    }

    /// Kuma serves its dashboard with HTTP 200 on any path that is not /metrics, so a
    /// mistyped URL arrives here as a perfectly successful scrape of HTML.
    #[test]
    fn a_dashboard_html_response_is_a_failure() {
        assert!(aggregate("<!doctype html><html><body>Uptime Kuma</body></html>").is_none());
    }

    #[test]
    fn a_response_with_only_other_metrics_is_a_failure() {
        assert!(aggregate("process_cpu_seconds_total 1.5\nnodejs_version_info 1\n").is_none());
    }

    /// A monitor list that is genuinely empty still counts as a working scrape, so long as
    /// Kuma said something about itself.
    #[test]
    fn a_real_kuma_response_with_no_monitors_still_counts() {
        let m = aggregate(r#"uptime_kuma_certificate_valid{monitor_name="a"} 1"#)
            .expect("recognisably Kuma");
        assert_eq!(value(&m, "kuma_monitors_total"), 0.0);
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        assert!(parse("# HELP x y\n\n# TYPE x gauge\n").is_empty());
    }

    #[test]
    fn a_trailing_timestamp_is_not_mistaken_for_the_value() {
        let parsed = parse(r#"m{a="b"} 5 1690387200000"#);
        assert_eq!(parsed[0].value, 5.0);
    }

    #[test]
    fn a_metric_without_labels_parses() {
        let parsed = parse("plain_metric 7");
        assert_eq!(parsed[0].name, "plain_metric");
        assert_eq!(parsed[0].value, 7.0);
    }

    #[test]
    fn labels_are_split_into_pairs() {
        let parsed = parse(r#"m{a="1",b="two"} 3"#);
        assert_eq!(
            parsed[0].labels,
            vec![("a", "1".to_string()), ("b", "two".to_string())]
        );
    }

    /// A monitor named with a quote would otherwise truncate the label and could drop the
    /// sample entirely, silently changing the count.
    #[test]
    fn an_escaped_quote_does_not_end_a_label_early() {
        let parsed = parse(r#"m{name="say \"hi\"",other="x"} 1"#);
        assert_eq!(parsed[0].labels[0].1, r#"say "hi""#);
        assert_eq!(parsed[0].labels[1].0, "other");
    }

    #[test]
    fn a_malformed_line_is_skipped_rather_than_panicking() {
        let parsed = parse("this is not a metric\nm{a=\"b\"} 1\n");
        assert_eq!(parsed.len(), 1);
    }
}
