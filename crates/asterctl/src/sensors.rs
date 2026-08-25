// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: Copyright (c) 2025 Markus Zehnder

//! Sensor value sources.
//!
//! Implementations:
//! - internal date time sensors
//! - file-based value provider with simple key-value pairs.

use crate::store::SensorStore;
use chrono::{DateTime, Datelike, Local, Timelike};
use log::{debug, error, info, warn};
use notify::event::{ModifyKind, RenameMode};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::ops::DerefMut;
use std::path::{Path, PathBuf};
use std::process::exit;
use std::sync::{Arc, RwLock, mpsc};
use std::time::{Duration, Instant, SystemTime};

pub fn get_date_time_value(label: &str, now: &DateTime<Local>) -> Option<String> {
    if !label.starts_with("DATE_") {
        return None;
    }

    let year = now.year();
    let month = format!("{:02}", now.month());
    let day = format!("{:02}", now.day());
    let hour = format!("{:02}", now.hour());
    let minute = format!("{:02}", now.minute());
    let second = format!("{:02}", now.second());

    // same formatting logic as in AOOSTAR-X
    let value = match label {
        "DATE_year" => year.to_string(),
        "DATE_month" => month,
        "DATE_day" => day,
        "DATE_hour" => hour,
        "DATE_minute" => minute,
        "DATE_second" => second,
        "DATE_m_d_h_m_1" => format!("{month}月{day}日  {hour}:{minute}"),
        "DATE_m_d_h_m_2" => format!("{month}/{day}  {hour}:{minute}"),
        "DATE_m_d_1" => format!("{month}月{day}日"),
        "DATE_m_d_2" => format!("{month}-{day}"),
        "DATE_y_m_d_1" => format!("{year}年{month}月{day}日"),
        "DATE_y_m_d_2" => format!("{year}-{month}-{day}"),
        "DATE_y_m_d_3" => format!("{year}/{month}/{day}"),
        "DATE_y_m_d_4" => format!("{year} {month} {day}"),
        "DATE_h_m_s_1" => format!("{hour}:{minute}:{second}"),
        "DATE_h_m_s_2" => format!("{hour}时{minute}分{second}秒"),
        "DATE_h_m_s_3" => format!("{hour} {minute} {second}"),
        "DATE_h_m_1" => format!("{hour}时{minute}分"),
        "DATE_h_m_2" => format!("{hour} : {minute}"),
        "DATE_h_m_3" => format!("{hour}:{minute}"),
        _ => return None,
    };

    Some(value)
}

/// Read all sensor value source files from the given path and stort monitoring for changes.
///
/// The source path is either a single sensor source file or a directory containing multiple sensor
/// source files.
///
/// The source path is monitored for changes in a separate thread.
/// All updated files are automatically read and stored in the shared HashMap.
///
/// # Arguments
///
/// * `source_path`: Single source file path or a directory path.
/// * `values`: a shared, reader-writer lock protected HashMap
/// * `sensor_filter`: Optional list of regex filters to filter out matching sensor keys.
///
/// returns: Result<(), Error>
pub fn start_file_slurper<P: Into<PathBuf>>(
    source_path: P,
    values: Arc<RwLock<SensorStore>>,
    sensor_filter: Option<Vec<Regex>>,
) -> anyhow::Result<()> {
    let dir_path = source_path.into();
    // read existing file(s)
    {
        let mut val = values.write().expect("Failed to lock values");
        read_path(&dir_path, val.deref_mut(), sensor_filter.as_deref())?;
    }

    let file_values = values.clone();

    std::thread::spawn(move || {
        // watch sensor file/directory for changes
        let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
        let mut watcher = match notify::recommended_watcher(tx) {
            Ok(w) => w,
            Err(e) => {
                error!("Failed to initialize watcher: {e}");
                exit(1);
            }
        };

        info!("Starting sensor file watcher for {dir_path:?} with filter {sensor_filter:?}");
        if let Err(e) = watcher.watch(&dir_path, RecursiveMode::NonRecursive) {
            error!("Failed to start file watcher: {e}");
            exit(1);
        }

        // Block forever, printing out events as they come in
        for res in rx {
            let event = match res {
                Ok(event) => event,
                Err(e) => {
                    warn!("watch error: {e:?}");
                    continue;
                }
            };
            match event.kind {
                EventKind::Modify(kind)
                    if matches!(kind, ModifyKind::Data(_) | ModifyKind::Name(RenameMode::To)) =>
                {
                    for path in event.paths.iter() {
                        if path.extension().unwrap_or_default() != "txt" {
                            continue;
                        }
                        debug!("Modified sensor file ({kind:?}): {path:?}");
                        let mut val = file_values.write().expect("Poisoned sensor RwLock");

                        if let Err(e) =
                            read_sensor_file(path, val.deref_mut(), sensor_filter.as_deref())
                        {
                            warn!("Failed to read sensor file {path:?}: {e}");
                            continue;
                        }
                    }
                }
                _ => {
                    // just for debugging
                    debug!("Watch event {:?}: {:?}", event.kind, event.paths);
                }
            }
        }
    });

    Ok(())
}

/// Read a single key-value-based source file or all source file for a given directory path.
///
/// # Arguments
///
/// * `path`: Single source file path or a directory path.
/// * `values`: HashMap to store all read key-value pairs.
/// * `sensor_filter`: Optional list of regex filters to filter out matching sensor keys.
///
/// returns: Result<(), Error>
fn read_path<P: AsRef<Path>>(
    path: P,
    store: &mut SensorStore,
    sensor_filter: Option<&[Regex]>,
) -> anyhow::Result<()> {
    let path = path.as_ref();

    if !path.try_exists()? {
        return Ok(());
    }

    if path.is_file() {
        return read_sensor_file(path, store, sensor_filter);
    }

    for entry in fs::read_dir(path)? {
        let path = entry?.path();

        if path.is_file()
            && path.extension().unwrap_or_default() == "txt"
            && let Err(e) = read_sensor_file(&path, store, sensor_filter)
        {
            warn!("Failed to read sensor file {path:?}: {e}");
        }
    }

    Ok(())
}

/// Read a sensor source file into the timestamped [`SensorStore`].
///
/// The file stem identifies the source (`host.txt` → `host`), so every provider gets its own
/// age tracking and can be reported on independently. All values from one read share a single
/// timestamp.
///
/// # Arguments
///
/// * `path`: sensor source file to read.
/// * `store`: store to insert the values into.
/// * `sensor_filter`: Optional list of regex filters to filter out matching sensor keys.
pub fn read_sensor_file<P: AsRef<Path>>(
    path: P,
    store: &mut SensorStore,
    sensor_filter: Option<&[Regex]>,
) -> anyhow::Result<()> {
    let path = path.as_ref();
    let source_name = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".to_string());

    let source = store.source_id(&source_name);

    // Age is taken from the file's modification time, not from the moment we happen to read
    // it. Stamping the read time would make every value look fresh immediately after start-up
    // — so a crash-restarted renderer would present hours-old numbers as live, which is the
    // exact failure staleness handling exists to prevent.
    //
    // The stamp stays an Instant so comparisons remain monotonic and immune to clock steps;
    // only the initial offset comes from the wall clock.
    let updated = Instant::now()
        .checked_sub(file_age(path))
        .unwrap_or_else(Instant::now);

    for (key, value) in parse_key_value_file(path, sensor_filter)? {
        store.insert(key, value, source, updated);
    }

    Ok(())
}

/// How long ago a file was last modified, or zero if that cannot be determined.
fn file_age(path: &Path) -> Duration {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .map(|modified| age_between(modified, SystemTime::now()))
        .unwrap_or_default()
}

/// Elapsed time between two wall-clock instants, clamped at zero.
///
/// A modification time in the future — clock skew, or a file touched by another host — yields
/// zero rather than an error, so the value is treated as fresh instead of being discarded.
fn age_between(modified: SystemTime, now: SystemTime) -> Duration {
    now.duration_since(modified).unwrap_or_default()
}

/// Read a key-value-based sensor source file and store content in the provided hashmap.
///
/// - Empty lines are skipped
/// - Lines starting with # are skipped
/// - Key-value pairs must be separated by `:`
/// - All keys and values are trimmed
///
/// # Arguments
///
/// * `path`: file path to read.
/// * `values`: HashMap to insert key-value pairs from the file.
/// * `sensor_filter`: Optional list of regex filters to filter out matching sensor keys.
///
/// returns: Result<(), Error>
pub fn read_key_value_file<P: AsRef<Path>>(
    path: P,
    values: &mut HashMap<String, String>,
    sensor_filter: Option<&[Regex]>,
) -> anyhow::Result<()> {
    for (key, value) in parse_key_value_file(path, sensor_filter)? {
        values.insert(key, value);
    }

    Ok(())
}

/// Parse a key-value file into trimmed, filtered pairs.
///
/// Shared by the plain-map reader used for configuration files and the [`SensorStore`]
/// reader used for sensor values.
///
/// - Empty lines are skipped
/// - Lines starting with # are skipped
/// - Key-value pairs must be separated by `:`
/// - All keys and values are trimmed
fn parse_key_value_file<P: AsRef<Path>>(
    path: P,
    sensor_filter: Option<&[Regex]>,
) -> anyhow::Result<Vec<(String, String)>> {
    debug!("Reading sensor file {:?}", path.as_ref());

    let mut pairs = Vec::new();
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);

    for line in reader.lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            if let Some(filter) = sensor_filter
                && is_filtered(key, filter)
            {
                debug!("Filtered: {key}");
                continue;
            }

            pairs.push((key.trim().to_string(), value.trim().to_string()));
        } else {
            warn!("Skipping invalid entry in sensor value file: {line}");
        }
    }

    Ok(pairs)
}

fn is_filtered(key: &str, filters: &[Regex]) -> bool {
    filters.iter().any(|re| re.is_match(key))
}

/// Read the sensor filter configuration file.
///
/// This is a simple text file containing multiple RegEx expressions.
/// - one RegEx per line
/// - Empty lines are skipped
/// - Lines starting with # are skipped
///
/// # Arguments
///
/// * `path`: file path to read.
///
/// returns: None if the file is empty or contains no valid RegEx expressions.
///
pub fn read_filter_file<P: AsRef<Path>>(path: P) -> anyhow::Result<Option<Vec<Regex>>> {
    debug!("Reading sensor filter file {:?}", path.as_ref());

    let mut filters = Vec::new();
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);

    for line in reader.lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        match Regex::new(line) {
            Ok(re) => {
                filters.push(re);
            }
            Err(e) => {
                warn!("Skipping invalid filter in sensor filter file: {line}: {e}");
            }
        }
    }

    if filters.is_empty() {
        Ok(None)
    } else {
        Ok(Some(filters))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{SensorStore, StalenessConfig};
    use rstest::rstest;

    #[test]
    fn age_between_measures_elapsed_wall_clock() {
        let now = SystemTime::now();
        let modified = now - Duration::from_secs(90);

        assert_eq!(age_between(modified, now), Duration::from_secs(90));
    }

    #[test]
    fn age_between_clamps_a_future_modification_time_to_zero() {
        let now = SystemTime::now();
        let modified = now + Duration::from_secs(600);

        assert_eq!(
            age_between(modified, now),
            Duration::ZERO,
            "clock skew must read as fresh, not as an error"
        );
    }

    /// Regression guard: an old file must be stale the moment it is first read. Stamping the
    /// read time instead of the modification time would make every restart resurrect dead data.
    #[test]
    fn an_old_sensor_file_is_stale_on_first_read() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("host.txt");
        std::fs::write(&path, "cpu_usage_percent: 42\n").expect("write");

        let hour_ago = SystemTime::now() - Duration::from_secs(3600);
        let file = fs::File::options().write(true).open(&path).expect("open");
        file.set_times(fs::FileTimes::new().set_modified(hour_ago))
            .expect("set mtime");

        let mut store = SensorStore::new(StalenessConfig::new(Some(Duration::from_secs(10))));
        read_sensor_file(&path, &mut store, None).expect("read");

        assert_eq!(
            store.resolve("cpu_usage_percent", Instant::now()),
            None,
            "an hour-old file must not resolve as a live value"
        );
    }

    #[test]
    fn a_just_written_sensor_file_is_live() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("host.txt");
        std::fs::write(&path, "cpu_usage_percent: 42\n").expect("write");

        let mut store = SensorStore::new(StalenessConfig::new(Some(Duration::from_secs(10))));
        read_sensor_file(&path, &mut store, None).expect("read");

        assert_eq!(
            store.resolve("cpu_usage_percent", Instant::now()),
            Some("42")
        );
    }

    #[test]
    fn is_filtered_does_not_filter_without_filters() {
        let key = "foobar";
        let filters = Vec::new();
        assert!(!is_filtered(key, &filters));
    }

    #[test]
    fn test_unit_extension_filter() {
        let key = "temperature_cpu#unit";
        let filters = vec![Regex::new("^temperature_.*#unit").unwrap()];
        assert!(is_filtered(key, &filters));
    }

    #[rstest]
    #[case(vec!["^foo$"])]
    #[case(vec!["^bar"])]
    #[case(vec!["other"])]
    #[case(vec!["123", "bla", "other"])]
    fn is_filtered_does_not_filter_without_a_match(#[case] filters: Vec<&str>) {
        let key = "foobar";
        let filters: Vec<Regex> = filters
            .iter()
            .map(|f| Regex::new(f).expect("Invalid regex"))
            .collect();
        assert!(
            !is_filtered(key, &filters),
            "Filter {filters:?} should not match {key}"
        );
        //
    }

    #[rstest]
    #[case(vec!["foo"])]
    #[case(vec!["bar"])]
    #[case(vec!["^.+bar"])]
    #[case(vec!["123", "foo", "other"])]
    #[case(vec!["bar", "123"])]
    #[case(vec!["^.+bar", "other"])]
    fn is_filtered_matches_filters(#[case] filters: Vec<&str>) {
        let key = "foobar";
        let filters: Vec<Regex> = filters
            .iter()
            .map(|f| Regex::new(f).expect("Invalid regex"))
            .collect();
        assert!(
            is_filtered(key, &filters),
            "Filter {filters:?} match match {key}"
        );
    }
}
