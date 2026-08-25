// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: Copyright (c) 2025 Markus Zehnder

//! Visual verification for staleness handling.
//!
//! Renders the stock panel in three states — everything live, one source dead, everything
//! dead — and saves the results to `out/` for inspection. Runs headless, no hardware.
//!
//! The assertions cover what can be checked mechanically: that rendering succeeds, and that
//! the three states actually produce different images. Whether the result *reads* correctly
//! is a judgement call made by looking at the PNGs.

use asterctl::cfg;
use asterctl::render::PanelRenderer;
use asterctl::store::{SensorStore, StalenessConfig};
use asterctl_lcd::DISPLAY_SIZE;

use std::path::Path;
use std::time::{Duration, Instant};

const CFG_DIR: &str = "../../cfg";
const FONT_DIR: &str = "../../fonts";
const OUT_DIR: &str = "../../out";

/// Panel 1 sensors, split across two providers so a partial failure is visible.
const HOST_VALUES: &[(&str, &str)] = &[
    ("cpu_temperature", "65"),
    ("cpu_percent", "47.7"),
    ("memory_usage", "77"),
    ("memory_Temperature", "48"),
];

const HOMELAB_VALUES: &[(&str, &str)] = &[
    ("net_ip_address", "192.168.68.24"),
    ("gpu_core", "98"),
    ("gpu_temperature", "78"),
    ("net_upload_speed", "100"),
    ("net_download_speed", "120"),
];

const MAX_AGE_SECS: u64 = 10;

/// Build a store where each source's values are the given age.
fn store_with_ages(now: Instant, host_age: Duration, homelab_age: Duration) -> SensorStore {
    let mut store = SensorStore::new(StalenessConfig::new(Some(Duration::from_secs(
        MAX_AGE_SECS,
    ))));

    let host = store.source_id("host");
    for (key, value) in HOST_VALUES {
        store.insert(key.to_string(), value.to_string(), host, now - host_age);
    }

    let homelab = store.source_id("homelab");
    for (key, value) in HOMELAB_VALUES {
        store.insert(
            key.to_string(),
            value.to_string(),
            homelab,
            now - homelab_age,
        );
    }

    store
}

fn render_state(name: &str, store: &SensorStore) -> image::RgbaImage {
    let mut config = cfg::load_cfg(Path::new(CFG_DIR).join("monitor.json"))
        .expect("stock monitor.json should load");
    let panel = config
        .get_next_active_panel()
        .expect("stock config should have an active panel");

    let mut renderer = PanelRenderer::new(DISPLAY_SIZE, FONT_DIR, CFG_DIR);
    renderer.set_img_save_path(OUT_DIR);
    renderer.set_img_suffix(format!("-stale-{name}"));
    renderer.set_save_render_img(true);

    renderer
        .render(panel, store)
        .unwrap_or_else(|e| panic!("rendering '{name}' should succeed: {e:?}"))
}

#[test]
fn stale_states_render_and_differ() {
    std::fs::create_dir_all(OUT_DIR).expect("out dir should be creatable");
    let now = Instant::now();
    let fresh = Duration::from_secs(1);
    let dead = Duration::from_secs(MAX_AGE_SECS * 6);

    // 1. everything live — the normal case
    let all_live = render_state("1-all-live", &store_with_ages(now, fresh, fresh));

    // 2. one provider dead — its values must drop to placeholders while the other keeps
    //    reporting, which is the local-fallback behaviour the split architecture relies on
    let one_dead = render_state("2-homelab-dead", &store_with_ages(now, fresh, dead));

    // 3. every provider dead — the clock still ticks, so the panel is visibly alive but
    //    visibly without data
    let all_dead = render_state("3-all-dead", &store_with_ages(now, dead, dead));

    assert_ne!(
        all_live.as_raw(),
        one_dead.as_raw(),
        "a dead provider must change what is drawn"
    );
    assert_ne!(
        one_dead.as_raw(),
        all_dead.as_raw(),
        "losing the second provider must change what is drawn"
    );
}

#[test]
fn live_values_are_resolved_for_rendering() {
    let now = Instant::now();
    let store = store_with_ages(now, Duration::from_secs(1), Duration::from_secs(1));

    assert_eq!(store.resolve("cpu_temperature", now), Some("65"));
    assert_eq!(store.resolve("gpu_core", now), Some("98"));
    assert_eq!(store.sources_live(now), 2);
}

#[test]
fn dead_provider_values_resolve_to_none_while_the_other_survives() {
    let now = Instant::now();
    let store = store_with_ages(
        now,
        Duration::from_secs(1),
        Duration::from_secs(MAX_AGE_SECS * 6),
    );

    assert_eq!(
        store.resolve("cpu_temperature", now),
        Some("65"),
        "the live provider is unaffected"
    );
    assert_eq!(
        store.resolve("gpu_core", now),
        None,
        "the dead provider's values must not render as live"
    );
    assert_eq!(store.sources_live(now), 1);
    assert_eq!(
        store.sys_value("SYS_sources_health", now).as_deref(),
        Some("50")
    );
}

/// The clock is generated at render time, so it survives total provider failure. That is what
/// separates "renderer alive, data stale" from "everything dead" on the panel.
#[test]
fn date_time_sensors_survive_total_failure() {
    let now = Instant::now();
    let store = store_with_ages(
        now,
        Duration::from_secs(MAX_AGE_SECS * 6),
        Duration::from_secs(MAX_AGE_SECS * 6),
    );

    assert_eq!(store.sources_live(now), 0);
    assert_eq!(
        store.sys_value("SYS_sources_health", now).as_deref(),
        Some("0")
    );

    let local_now = chrono::Local::now();
    assert!(
        asterctl::sensors::get_date_time_value("DATE_m_d_h_m_2", &local_now).is_some(),
        "the clock must keep working with every provider dead"
    );
}
