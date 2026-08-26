// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: Copyright (c) 2025 Markus Zehnder

#![forbid(non_ascii_idents)]
#![deny(unsafe_code)]

use asterctl::cfg::{MonitorConfig, Panel, load_custom_panel};
use asterctl::render::PanelRenderer;
use asterctl::scrub::{self, DEFAULT_SCRUB_DURATION, PixelShift, ScrubSequence};
use asterctl::sensors::{SensorFilter, read_filter_file, read_key_value_file, start_file_slurper};
use asterctl::store::{SensorStore, StalenessConfig, parse_max_age_entries};
use asterctl::{cfg, img};
use asterctl_lcd::{AooScreen, AooScreenBuilder, DISPLAY_SIZE};

use anyhow::anyhow;
use clap::Parser;
use env_logger::Env;
use log::{debug, error, info, warn};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use std::thread::sleep;
use std::time::{Duration, Instant};

/// AOOSTAR WTR MAX and GEM12+ PRO screen control.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Serial device, for example, "/dev/cu.usbserial-AB0KOHLS". Takes priority over --usb option.
    #[arg(short, long)]
    device: Option<String>,

    /// USB serial UART "vid:pid" in hex notation (lsusb output). Default: 416:90A1
    #[arg(short, long)]
    usb: Option<String>,

    /// Switch display on and exit. This will show the last displayed image.
    #[arg(long)]
    on: bool,

    /// Switch display off and exit.
    #[arg(long)]
    off: bool,

    /// Image to display, other sizes than 960x376 will be scaled.
    #[arg(short, long)]
    image: Option<String>,

    /// AOOSTAR-X json configuration file to parse.
    ///
    /// The configuration file will be loaded from the `config_dir` directory if no full path is
    /// specified.
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Include one or more additional custom panels into the base configuration.
    ///
    /// Specify the path to the panel directory containing panel.json and fonts / img subdirectories.
    #[arg(short, long)]
    panels: Option<Vec<PathBuf>>,

    /// Configuration directory containing configuration files and background images
    /// specified in the `config` file.
    #[arg(long, default_value_t = String::from("cfg"))]
    config_dir: String, // default_value_t requires Display trait which PathBuf does not implement

    /// Font directory for fonts specified in the `config` file.
    #[arg(long, default_value_t = String::from("fonts"))]
    font_dir: String,

    /// Single sensor value input file or directory for multiple sensor input files.
    #[arg(long, default_value_t = String::from("cfg/sensors"))]
    sensor_path: String,

    /// Sensor identifier mapping file. Ignored if the file does not exist.
    ///
    /// The configuration file will be loaded from the `config_dir` directory if no full path is
    /// specified.
    #[arg(long, default_value_t = String::from("sensor-mapping.cfg"))]
    sensor_mapping: String,

    /// Maximum age in seconds before a sensor value is treated as absent.
    ///
    /// A provider that dies leaves its last values behind, and without a maximum age they
    /// keep rendering as though live. Past this age a value renders as a placeholder instead.
    ///
    /// Unset disables staleness handling entirely: values never expire.
    #[arg(long)]
    max_age: Option<u64>,

    /// Per-source maximum age overrides, one `<sensor file stem>: <seconds>` pair per line.
    ///
    /// Loaded from the `config_dir` directory if no full path is specified.
    /// Ignored if the file does not exist.
    #[arg(long, default_value_t = String::from("max-age.cfg"))]
    max_age_file: String,

    /// Run a retention scrub every N minutes. Unset disables it.
    ///
    /// This panel holds a static image indefinitely, and a sensor layout never moves, so
    /// the display gradually retains a ghost of whatever it has been showing. A scrub
    /// drives every pixel through its full range to break that up.
    ///
    /// Each scrub frame changes the whole screen, so it costs a full transfer — about 1.3s
    /// on this hardware. Twenty seconds an hour is well under 1% of the link.
    #[arg(long)]
    scrub_interval: Option<u64>,

    /// How long each retention scrub runs, in seconds. Default 20.
    #[arg(long)]
    scrub_duration: Option<u64>,

    /// Offset the whole panel by up to N pixels, moving on each maintenance cycle.
    ///
    /// The scrub treats retention that has already happened; this prevents the layout from
    /// creating more. A tile edge that sits on exactly the same pixels for years etches
    /// itself in however good the conditioning is — moving by a pixel or two spreads that
    /// wear over a neighbourhood.
    ///
    /// Timed by `--scrub-interval`, so with both enabled the shift is free: the frame after
    /// a scrub is a full redraw regardless. Requires `--scrub-interval` to be set.
    ///
    /// 1 or 2 is plenty. Larger values become visible as the panel moving.
    #[arg(long)]
    pixel_shift: Option<i32>,

    /// Anchor scrubs to this minute past the hour, so they happen at a predictable time.
    ///
    /// Without it the schedule is measured from whenever the service last started, which
    /// puts the scrub at an arbitrary and drifting time of day. Anchored, you know when the
    /// panel will blank — so a screen of test patterns reads as scheduled maintenance
    /// rather than as a fault.
    ///
    /// Use with a whole-hour `--scrub-interval`, or successive scrubs drift off the anchor.
    #[arg(long, value_name = "MINUTE")]
    scrub_at_minute: Option<u32>,

    /// File recording when the last retention scrub ran, so the schedule survives a
    /// restart.
    ///
    /// The in-process timer resets on every restart, so a deployment or a crash loop can
    /// starve the display of conditioning indefinitely — three restarts during one deploy
    /// pushed the next scrub out by three full intervals.
    ///
    /// Put it on tmpfs: surviving a restart is the point, surviving a reboot is not.
    #[arg(long, value_name = "FILE")]
    scrub_state_file: Option<PathBuf>,

    /// Switch off display n seconds after loading image or running demo.
    #[arg(short, long)]
    off_after: Option<u32>,

    /// Test mode: only write to the display without checking response.
    #[arg(short, long)]
    write_only: bool,

    /// Test mode: save changed images in ./out folder.
    #[arg(short, long)]
    save: bool,

    /// Simulate serial port for testing and development, `--device` and `--usb` options are ignored.
    #[arg(long)]
    simulate: bool,
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();

    let args = Args::parse();

    // initialize display with given UART port parameter
    let mut builder = AooScreenBuilder::new();
    builder.no_init_check(args.write_only);
    let mut screen = if args.simulate {
        builder.simulate()?
    } else if let Some(device) = args.device {
        builder.open_device(&device)?
    } else if let Some(usb) = args.usb {
        builder.open_usb_id(&usb)?
    } else {
        builder.open_default()?
    };

    // process simple commands
    if args.off {
        screen.off()?;
        return Ok(());
    } else if args.on {
        screen.on()?;
        return Ok(());
    }

    // switch on screen for remaining commands
    screen.init()?;

    if let Some(config) = args.config {
        info!("Starting sensor panel mode");
        let img_save_path = if args.save {
            let img_save_path = PathBuf::from("out");
            fs::create_dir_all(&img_save_path)?;
            Some(img_save_path)
        } else {
            None
        };

        let cfg_dir = PathBuf::from(args.config_dir);
        let font_dir = PathBuf::from(args.font_dir);
        let sensor_path = PathBuf::from(args.sensor_path);
        let mapping_cfg = PathBuf::from(args.sensor_mapping);
        let staleness = load_staleness(args.max_age, Path::new(&args.max_age_file), &cfg_dir)?;
        let cfg = load_configuration(&config, &cfg_dir, args.panels, &mapping_cfg)?;
        run_sensor_panel(
            &mut screen,
            cfg,
            PanelPaths {
                config_dir: cfg_dir,
                font_dir,
                sensor_path,
                img_save_path,
            },
            staleness,
            Maintenance {
                interval: args.scrub_interval.map(|m| Duration::from_secs(m * 60)),
                scrub_duration: args
                    .scrub_duration
                    .map(Duration::from_secs)
                    .unwrap_or(DEFAULT_SCRUB_DURATION),
                pixel_shift_max: args.pixel_shift.unwrap_or(0),
                state_file: args.scrub_state_file,
                at_minute: args.scrub_at_minute,
            },
        )?;
        return Ok(());
    }

    if let Some(image) = args.image {
        info!("Loading and displaying background image {image}...");
        let rgb_img = img::load_image(&image, Some(DISPLAY_SIZE))?.to_rgb8();
        let timestamp = Instant::now();
        screen.send_image(&rgb_img)?;
        debug!("Image sent in {}ms", timestamp.elapsed().as_millis());
    }

    if let Some(off) = args.off_after {
        info!("Switching off display in {off}s");
        sleep(Duration::from_secs(off as u64));
        screen.off()?;
    }

    info!("Bye bye!");

    Ok(())
}

fn load_configuration<P: AsRef<Path>>(
    config: P,
    config_dir: P,
    panels: Option<Vec<PathBuf>>,
    sensor_mapping: P,
) -> anyhow::Result<MonitorConfig> {
    let config = config.as_ref();
    let config_dir = config_dir.as_ref();

    let mut cfg = if config.is_absolute() {
        cfg::load_cfg(config)?
    } else {
        cfg::load_cfg(config_dir.join(config))?
    };

    if let Some(panels) = panels {
        for panel in panels {
            cfg.include_custom_panel(load_custom_panel(panel)?);
        }
    }

    let sensor_mapping = sensor_mapping.as_ref();
    let mapping_cfg = if sensor_mapping.is_absolute() {
        sensor_mapping.to_path_buf()
    } else {
        config_dir.join(sensor_mapping)
    };
    if mapping_cfg.is_file() {
        let mut mapping = HashMap::new();
        read_key_value_file(&mapping_cfg, &mut mapping, None)?;
        cfg.set_sensor_mapping(mapping);
    } else {
        info!("Sensor mapping file {mapping_cfg:?} not found");
    }

    cfg.sensor_filter = load_sensor_filter(&mapping_cfg)?;

    Ok(cfg)
}

/// Build the staleness configuration from the global maximum age and optional per-source file.
///
/// Returns a disabled configuration when neither is set, in which case sensor values never
/// expire and rendering behaves exactly as it did before staleness handling existed.
fn load_staleness(
    max_age: Option<u64>,
    max_age_file: &Path,
    config_dir: &Path,
) -> anyhow::Result<StalenessConfig> {
    let mut staleness = StalenessConfig::new(max_age.map(Duration::from_secs));

    let path = if max_age_file.is_absolute() {
        max_age_file.to_path_buf()
    } else {
        config_dir.join(max_age_file)
    };

    if path.is_file() {
        info!("Loading per-source max age file {path:?}");
        let mut entries = HashMap::new();
        read_key_value_file(&path, &mut entries, None)?;
        parse_max_age_entries(&entries, &mut staleness)?;
    } else {
        info!("No per-source max age file {path:?} available");
    }

    if staleness.enabled() {
        info!("Staleness handling enabled");
    } else {
        info!("Staleness handling disabled: sensor values never expire");
    }

    Ok(staleness)
}

fn load_sensor_filter(mapping_cfg: &Path) -> anyhow::Result<Option<SensorFilter>> {
    if let Some(parent) = mapping_cfg.parent()
        && let Some(file_stem) = mapping_cfg.file_stem()
        && let Some(extension) = mapping_cfg.extension()
    {
        let filter_file = parent
            .join(format!("{}-filter", file_stem.to_string_lossy()))
            .with_extension(extension);

        if filter_file.is_file() {
            info!("Loading sensor filter file {filter_file:?}");
            return read_filter_file(filter_file);
        } else {
            info!("No sensor filter file {filter_file:?} available");
        }
    }

    Ok(None)
}

/// Directories the sensor panel reads from, and where to save debug renders.
struct PanelPaths {
    /// Configuration files and background images.
    config_dir: PathBuf,
    /// TTF fonts referenced by panel definitions.
    font_dir: PathBuf,
    /// Sensor value file, or a directory of them.
    sensor_path: PathBuf,
    /// Debug only: where to write rendered PNGs.
    img_save_path: Option<PathBuf>,
}

/// Display upkeep that runs between panels rather than as part of rendering.
///
/// The two measures share a schedule on purpose: a scrub ends with a full-frame redraw, so
/// advancing the pixel offset at the same moment costs nothing extra.
struct Maintenance {
    /// How often to run upkeep. `None` disables both measures.
    interval: Option<Duration>,
    /// Minimum time each conditioning scrub runs for.
    scrub_duration: Duration,
    /// Maximum whole-panel offset in pixels. Zero disables the shift.
    pixel_shift_max: i32,
    /// Anchor scrubs to this minute past the hour instead of measuring from startup.
    at_minute: Option<u32>,
    /// Optional file recording the last scrub, so the schedule survives a restart.
    state_file: Option<PathBuf>,
}

fn run_sensor_panel(
    screen: &mut AooScreen,
    mut cfg: MonitorConfig,
    paths: PanelPaths,
    staleness: StalenessConfig,
    maintenance: Maintenance,
) -> anyhow::Result<()> {
    let PanelPaths {
        config_dir,
        font_dir,
        sensor_path,
        img_save_path,
    } = paths;

    let mut renderer = PanelRenderer::new(DISPLAY_SIZE, &font_dir, &config_dir);
    if let Some(img_save_path) = &img_save_path {
        renderer.set_img_save_path(img_save_path);
        renderer.set_save_render_img(true);
        // renderer.set_save_processed_pic(true);
        // renderer.set_save_progress_layer(true);
    }

    let sensor_values: Arc<RwLock<SensorStore>> =
        Arc::new(RwLock::new(SensorStore::new(staleness)));

    start_file_slurper(
        sensor_path,
        sensor_values.clone(),
        cfg.sensor_filter.clone(),
    )?;

    let refresh = Duration::from_millis((cfg.setup.refresh * 1000f32) as u64);

    let switch_time = cfg
        .setup
        .switch_time
        .as_deref()
        .and_then(|v| f32::from_str(v).ok())
        .map(|v| Duration::from_millis((v * 1000.0) as u64))
        .unwrap_or(Duration::from_secs(5));

    if let Some(interval) = maintenance.interval {
        info!(
            "Retention scrub enabled: {}s every {}min",
            maintenance.scrub_duration.as_secs(),
            interval.as_secs() / 60
        );
    }
    if maintenance.pixel_shift_max > 0 {
        if maintenance.interval.is_some() {
            info!(
                "Pixel shift enabled: up to {}px per maintenance cycle",
                maintenance.pixel_shift_max
            );
        } else {
            warn!("--pixel-shift needs --scrub-interval to schedule it; the panel will not move");
        }
    }
    // Two scheduling modes. Anchored to a minute past the hour is preferred: it is
    // predictable, and it survives restarts for free because the schedule is absolute
    // rather than measured from process start.
    let mut next_scrub: Option<chrono::DateTime<chrono::Local>> = None;
    if let (Some(minute), Some(interval)) = (maintenance.at_minute, maintenance.interval) {
        if !interval.as_secs().is_multiple_of(3600) {
            warn!(
                "--scrub-interval is not a whole number of hours, so scrubs will drift off \
                 minute {minute} after the first"
            );
        }
        next_scrub = scrub::next_scrub_time(chrono::Local::now(), minute, interval);
        match next_scrub {
            Some(at) => info!("Next scrub at {}", at.format("%H:%M")),
            None => warn!("Could not compute a scrub schedule; falling back to the interval"),
        }
    }

    // Interval mode only: carry the schedule across restarts, so a deploy that bounces the
    // service several times does not push the next scrub out by that many intervals.
    let mut last_scrub = Instant::now();
    if next_scrub.is_none()
        && let Some(state_file) = &maintenance.state_file
        && let Some(elapsed) = scrub::read_state(state_file)
    {
        info!("Last scrub was {}s ago", elapsed.as_secs());
        last_scrub = last_scrub.checked_sub(elapsed).unwrap_or(last_scrub);
    }
    let mut scrub_seed: u32 = 0x1234_5678;
    let mut pixel_shift = PixelShift::new(maintenance.pixel_shift_max);

    // panel switching loop
    loop {
        // Between panels, never mid-panel: a scrub blanks the screen, and interrupting a
        // panel to do it would look like a fault rather than maintenance.
        let due = match next_scrub {
            Some(at) => chrono::Local::now() >= at,
            None => maintenance
                .interval
                .is_some_and(|interval| last_scrub.elapsed() >= interval),
        };

        if due && let Some(interval) = maintenance.interval {
            scrub_seed = scrub_seed
                .wrapping_mul(1_664_525)
                .wrapping_add(1_013_904_223);
            run_scrub(screen, maintenance.scrub_duration, scrub_seed)?;
            last_scrub = Instant::now();
            if let Some(state_file) = &maintenance.state_file {
                scrub::write_state(state_file);
            }
            if let Some(minute) = maintenance.at_minute {
                next_scrub = scrub::next_scrub_time(chrono::Local::now(), minute, interval);
                if let Some(at) = next_scrub {
                    info!("Next scrub at {}", at.format("%H:%M"));
                }
            }

            // Advance after the scrub, not before: the next panel render is a full frame
            // either way, so moving the panel here costs nothing.
            if maintenance.pixel_shift_max > 0 {
                let offset = pixel_shift.advance();
                debug!("Pixel shift now {offset:?}");
                renderer.set_pixel_shift(offset);
            }
        }

        let panel = cfg
            .get_next_active_panel()
            .ok_or(anyhow!("No active panel"))?;

        info!("Switching panel: {}", panel.friendly_name());
        let panel_switch_time = Instant::now();

        // active panel refresh loop
        let mut refresh_count = 1;
        loop {
            let upd_start_time = Instant::now();

            if img_save_path.is_some() {
                renderer.set_img_suffix(format!("-{refresh_count:02}"));
            }

            // Keeping the read lock during panel rendering should be ok, otherwise we could always clone the HashMap
            let values = sensor_values.read().expect("RwLock is poisoned");
            update_panel(screen, &mut renderer, panel, &values)?;
            drop(values);

            let elapsed = upd_start_time.elapsed();
            if refresh > elapsed {
                sleep(refresh - elapsed);
            }

            if panel_switch_time.elapsed() >= switch_time {
                break;
            }

            refresh_count += 1;
        }
    }
}

/// Drive every pixel through its full range to break up a retained image.
///
/// Bounded by wall-clock time rather than a frame count, because each frame's transfer time
/// depends on the link and we care about how long the screen is unavailable, not how many
/// frames got sent.
fn run_scrub(screen: &mut AooScreen, duration: Duration, seed: u32) -> anyhow::Result<()> {
    info!("Starting retention scrub for {}s...", duration.as_secs());
    let start = Instant::now();
    let mut frames = 0u32;
    let mut sequence = ScrubSequence::new(DISPLAY_SIZE, seed);

    // `duration` is a minimum, not a deadline. A partial cycle conditions the display
    // unevenly — cut off early it might show only white and black, never touching the
    // individual colour channels or the noise pass — so always finish the cycle in
    // progress. At roughly 1.3s per full-frame transfer, one cycle is about 13 seconds.
    loop {
        if start.elapsed() >= duration && sequence.at_cycle_start() {
            break;
        }
        let Some(image) = sequence.next() else {
            break;
        };
        screen.send_image(&image)?;
        frames += 1;
    }

    if frames < ScrubSequence::cycle_len() as u32 {
        warn!(
            "Retention scrub sent only {frames} frames — the link is slower than expected \
             and conditioning will be uneven"
        );
    }

    info!(
        "Retention scrub finished: {frames} frames in {}ms",
        start.elapsed().as_millis()
    );

    Ok(())
}

fn update_panel(
    screen: &mut AooScreen,
    renderer: &mut PanelRenderer,
    panel: &Panel,
    values: &SensorStore,
) -> anyhow::Result<()> {
    debug!("Displaying panel '{}'...", panel.friendly_name());

    match renderer.render(panel, values) {
        Ok(image) => screen.send_image(&image)?,
        Err(e) => error!("Error rendering panel '{}': {e:?}", panel.friendly_name()),
    }

    Ok(())
}
