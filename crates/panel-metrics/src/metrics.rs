// SPDX-License-Identifier: MIT OR Apache-2.0

//! Prometheus text output.
//!
//! Deliberately hand-rolled rather than pulled from a metrics library. What this service
//! emits is a flat snapshot of gauges with no histograms, no registries and no process
//! instrumentation, and the consumer downstream is a 960x376 LCD panel. A library would add
//! a dependency and a lifecycle to manage in exchange for features nothing here uses.

use std::fmt::Write;

/// A single gauge sample.
#[derive(Debug, Clone, PartialEq)]
pub struct Metric {
    pub name: String,
    pub labels: Vec<(String, String)>,
    pub value: f64,
    pub help: Option<String>,
}

impl Metric {
    pub fn new(name: impl Into<String>, value: f64) -> Self {
        Self {
            name: name.into(),
            labels: Vec::new(),
            value,
            help: None,
        }
    }

    pub fn label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.labels.push((key.into(), value.into()));
        self
    }

    pub fn help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }
}

/// Escape a label value per the Prometheus exposition format.
///
/// Backslash, double quote and newline are the only characters that need it. A monitor or
/// pool name containing a quote would otherwise produce a response that fails to parse
/// downstream, which is a silent panel of `--` rather than an error.
fn escape_label(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out
}

/// Render metrics as Prometheus text exposition format.
///
/// `# HELP` is emitted once per metric name, on first appearance, since repeating it for
/// every labelled sample is invalid.
pub fn render(metrics: &[Metric]) -> String {
    let mut out = String::new();
    let mut described: Vec<&str> = Vec::new();

    for metric in metrics {
        if let Some(help) = &metric.help
            && !described.contains(&metric.name.as_str())
        {
            let _ = writeln!(out, "# HELP {} {}", metric.name, help);
            let _ = writeln!(out, "# TYPE {} gauge", metric.name);
            described.push(&metric.name);
        }

        if metric.labels.is_empty() {
            let _ = writeln!(out, "{} {}", metric.name, format_value(metric.value));
        } else {
            let labels: Vec<String> = metric
                .labels
                .iter()
                .map(|(k, v)| format!("{k}=\"{}\"", escape_label(v)))
                .collect();
            let _ = writeln!(
                out,
                "{}{{{}}} {}",
                metric.name,
                labels.join(","),
                format_value(metric.value)
            );
        }
    }

    out
}

/// Format a value the way Prometheus expects.
///
/// Whole numbers print without a decimal point, which keeps the common case of counts and
/// booleans readable, and non-finite values use the spelling the format requires rather
/// than Rust's.
fn format_value(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value.is_infinite() {
        return if value.is_sign_positive() {
            "+Inf"
        } else {
            "-Inf"
        }
        .to_string();
    }
    if value.fract() == 0.0 && value.abs() < 1e15 {
        return format!("{}", value as i64);
    }
    format!("{value}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_metric_renders_as_name_and_value() {
        let out = render(&[Metric::new("thing_total", 3.0)]);
        assert_eq!(out, "thing_total 3\n");
    }

    #[test]
    fn labels_are_rendered_in_order() {
        let out = render(&[Metric::new("t", 1.0).label("a", "x").label("b", "y")]);
        assert_eq!(out, "t{a=\"x\",b=\"y\"} 1\n");
    }

    #[test]
    fn help_is_emitted_once_per_name() {
        let out = render(&[
            Metric::new("t", 1.0).label("i", "1").help("a thing"),
            Metric::new("t", 2.0).label("i", "2").help("a thing"),
        ]);
        assert_eq!(out.matches("# HELP").count(), 1, "repeated HELP is invalid");
        assert_eq!(out.matches("# TYPE").count(), 1);
    }

    #[test]
    fn whole_numbers_lose_the_decimal_point() {
        assert_eq!(format_value(42.0), "42");
        assert_eq!(format_value(-7.0), "-7");
    }

    #[test]
    fn fractions_are_preserved() {
        assert_eq!(format_value(0.5), "0.5");
        assert_eq!(format_value(118.25), "118.25");
    }

    #[test]
    fn non_finite_values_use_prometheus_spelling() {
        assert_eq!(format_value(f64::NAN), "NaN");
        assert_eq!(format_value(f64::INFINITY), "+Inf");
        assert_eq!(format_value(f64::NEG_INFINITY), "-Inf");
    }

    /// A pool or monitor name containing a quote would otherwise emit an unparseable
    /// response, which reaches the panel as a screen of "--" rather than as an error.
    #[test]
    fn label_values_are_escaped() {
        let out = render(&[Metric::new("t", 1.0).label("name", "he said \"hi\"")]);
        assert_eq!(out, "t{name=\"he said \\\"hi\\\"\"} 1\n");
    }

    #[test]
    fn backslashes_and_newlines_are_escaped() {
        assert_eq!(escape_label("a\\b"), "a\\\\b");
        assert_eq!(escape_label("a\nb"), "a\\nb");
    }
}
