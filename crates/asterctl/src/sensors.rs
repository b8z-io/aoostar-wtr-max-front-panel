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

    match label {
        // see https://github.com/zehnm/aoostar-rs/issues/13
        "DATE_year" => Some(format!("{}", year)),
        "DATE_month" => Some(month),
        "DATE_day" => Some(day),
        "DATE_hour" => Some(hour),
        "DATE_minute" => Some(minute),
        "DATE_second" => Some(second),
        "DATE_y_m_d_1" => Some(format!("{year}-{month}-{day}")),
        "DATE_y_m_d_2" => Some(format!("{year}/{month}/{day}")),
        "DATE_y_m_d_3" => Some(format!("{year}.{month}.{day}")),
        "DATE_y_m_d_4" => Some(format!("{year}{month}{day}")),
        "DATE_m_d_1" => Some(format!("{month}-{day}")),
        "DATE_m_d_2" => Some(format!("{month}/{day}")),
        "DATE_m_d_h_m_1" => Some(format!("{month}-{day} {hour}:{minute}")),
        "DATE_m_d_h_m_2" => Some(format!("{month}/{day} {hour}:{minute}")),
        "DATE_h_m_s_1" => Some(format!("{hour}:{minute}:{second}")),
        "DATE_h_m_s_2" => Some(format!("{hour}:{minute}:{second}")),
        "DATE_h_m_s_3" => Some(format!("{hour}:{minute}:{second}")),
        "DATE_h_m_1" => Some(format!("{hour}:{minute}")),
        "DATE_h_m_2" => Some(format!("{hour}:{minute}")),
        "DATE_h_m_3" => Some(format!("{hour}:{minute}")),
        _ => None,
    }
}

/// Start sensor file watcher for a single source file or a directory.
///
/// Watches for sensor file changes and updates the shared sensor values map asynchronously.
///
/// # Arguments
///
/// * `source_path`: Single source file path or a directory path.
/// * `values`: a shared, reader-writer lock protected SensorStore
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

                        if let Err(e) = read_sensor_file(path, val.deref_mut(), sensor_filter.as_deref())
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
/// * `values`: SensorStore to store all read key-value pairs.
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
        }
    }

    Ok(pairs)
}

/// Load a sensor filter file where each line is a regex pattern.
///
/// Lines starting with `!` are keep patterns, all others are drop patterns.
/// Returns `None` iff the file is empty or contains only comments/blank lines.
pub fn read_filter_file<P: AsRef<Path>>(path: P) -> anyhow::Result<Option<Vec<Regex>>> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut patterns = Vec::new();

    for line in reader.lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        patterns.push(Regex::new(line)?);
    }

    Ok(if patterns.is_empty() { None } else { Some(patterns) })
}

/// Check if a sensor key is filtered by any of the regex patterns.
fn is_filtered(key: &str, filter: &[Regex]) -> bool {
    filter.iter().any(|f| f.is_match(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_date_time_label() {
        let now: DateTime<Local> = Local::now();
        let year = now.year();

        assert_eq!(
            get_date_time_value("DATE_h_m_s_1", &now),
            Some(format!("{:02}:{:02}:{:02}", now.hour(), now.minute(), now.second()))
        );
        assert_eq!(
            get_date_time_value("DATE_y_m_d_1", &now),
            Some(format!("{}-{:02}-{:02}", year, now.month(), now.day()))
        );
        assert_eq!(get_date_time_value("DATE_year", &now), Some(format!("{year}")));
        assert_eq!(get_date_time_value("DATE_hour", &now), Some(format!("{:02}", now.hour())));
        assert_eq!(get_date_time_value("DATE_month", &now), Some(format!("{:02}", now.month())));
        assert_eq!(get_date_time_value("DATE_day", &now), Some(format!("{:02}", now.day())));
        assert_eq!(get_date_time_value("DATE_second", &now), Some(format!("{:02}", now.second())));
        assert_eq!(get_date_time_value("invalid", &now), None);
    }
}