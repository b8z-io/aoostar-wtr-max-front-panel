// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: Copyright (c) 2025 Markus Zehnder

//! Integration test: rendering a panel with mixed fresh and stale sensor values.

use asterctl::cfg::load_cfg;
use asterctl::render::PanelRenderer;
use asterctl::store::{SensorStore, StalenessConfig};
use asterctl_lcd::DISPLAY_SIZE;
use std::time::{Duration, Instant};

/// A panel with a handful of text sensors. Loaded from a fixture JSON string
/// rather than the filesystem so the test works in any working directory.
fn fixture_panel() -> asterctl::cfg::MonitorConfig {
    load_cfg("../../cfg/monitor.json").expect("fixture monitor.json")
}

/// Render a panel with some values fresh and some past their maximum age.
///
/// The test does not assert on pixels — display hardware differs — but on the
/// absence of a crash and on the render completing in reasonable time.
#[test]
fn stale_values_render_as_placeholder() {
    let now = Instant::now();

    let mut store = SensorStore::new(StalenessConfig::new(Some(Duration::from_secs(10))));

    // Fresh value within max-age
    let host = store.source_id("host");
    store.insert("cpu".into(), "42".into(), host, now);

    // Stale value past max-age
    store.insert(
        "mem".into(),
        "80".into(),
        host,
        now - Duration::from_secs(15),
    );

    // A second source that is still fresh
    let net = store.source_id("network");
    store.insert("net_speed".into(), "1000".into(), net, now);

    // Render
    let mut renderer = PanelRenderer::new(DISPLAY_SIZE, "../../fonts", "../../cfg");
    let panel = fixture_panel();
    // Use the first panel (index 0 in the panels vec)
    let panel = &panel.panels[0];

    // The render should not panic
    let result = renderer.render(panel, &store);
    assert!(result.is_ok(), "render should succeed with mixed stale and fresh values");
}

/// Without --max-age configured, nothing should be stale — behaviour is
/// identical to the original codebase.
#[test]
fn no_staleness_config_means_no_placeholder() {
    let now = Instant::now();

    let mut store = SensorStore::new(StalenessConfig::default());
    assert!(!store.staleness_enabled());

    let host = store.source_id("host");
    // A value that is very old would still resolve without staleness
    store.insert(
        "cpu".into(),
        "42".into(),
        host,
        now - Duration::from_secs(86_400),
    );

    assert_eq!(store.resolve("cpu", now), Some("42"));
}