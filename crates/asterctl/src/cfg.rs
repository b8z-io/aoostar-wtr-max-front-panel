// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: Copyright (c) 2025 Markus Zehnder

//! AOOSTAR-X json configuration file format.
//!
//! Derived from the available Monitor3.json file in AOOSTAR-X v1.3.4.
//! Likely not fully compatible with files created with the original editor.

use crate::sensors::SensorFilter;
use anyhow::Context;
use image::{Rgb, Rgba};
use imageproc::definitions::HasWhite;
use log::{info, warn};
use serde::de::Visitor;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_repr::{Deserialize_repr, Serialize_repr};
use std::collections::HashMap;
use std::io::BufReader;
use std::num::ParseIntError;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::{fmt, fs};

pub fn load_cfg<P: AsRef<Path>>(path: P) -> anyhow::Result<MonitorConfig> {
    let path = path.as_ref();
    let file = fs::File::open(path).with_context(|| format!("Failed to load config {path:?}"))?;
    let reader = BufReader::new(file);
    let config: MonitorConfig = serde_json::from_reader(reader)?;

    for active in config.active_panels.clone() {
        if active == 0 || active > config.panels.len() as u32 {
            warn!("Ignoring invalid active panel {active}");
            continue;
        }
        let panel = &config.panels[active as usize - 1];

        println!(
            "Panel {active}: {}",
            panel.img.as_deref().unwrap_or_default()
        );
        for sensor in &panel.sensor {
            println!(
                "  {}: {} {} {}",
                sensor.label,
                sensor
                    .name
                    .as_deref()
                    .or(sensor.item_name.as_deref())
                    .unwrap_or_default(),
                sensor.value.as_deref().unwrap_or_default(),
                sensor.unit.as_deref().unwrap_or_default()
            );
        }
    }

    Ok(config)
}

/// Load a custom panel configuration.
///
/// The distributed panel ZIP file must be extracted and contain:
/// - `panel.json` configuration file
/// - `img` subdirectory containing the referenced images in panel.json
/// - `fonts` subdirectory containing the referenced fonts in panel.json
///
/// # Arguments
///
/// * `path`: directory path of the extracted custom panel.
///
/// returns: Result<Panel, Error>
pub fn load_custom_panel<P: AsRef<Path>>(path: P) -> anyhow::Result<Panel> {
    let path = path.as_ref();
    let panel_file = path.join("panel.json");

    info!("Loading custom panel {panel_file:?}");

    let file = fs::File::open(&panel_file)
        .with_context(|| format!("Failed to load custom panel {panel_file:?}"))?;
    let reader = BufReader::new(file);
    let mut panel: Panel = serde_json::from_reader(reader)?;

    // adjust font and image file paths
    let img_path = fs::canonicalize(path.join("img"))?;
    let font_path = fs::canonicalize(path.join("fonts"))?;
    if let Some(img) = &panel.img
        && !Path::new(img).is_absolute()
    {
        panel.img = Some(img_path.join(img).display().to_string());
    }
    for sensor in panel.sensor.iter_mut() {
        if let Some(pic) = &sensor.pic
            && !Path::new(pic).is_absolute()
        {
            sensor.pic = Some(img_path.join(pic).display().to_string());
        }
        if let Some(font_family) = &sensor.font_family
            && !font_family.is_empty()
            && !Path::new(&font_family).is_absolute()
        {
            sensor.font_family = Some(font_path.join(font_family).display().to_string());
        }
    }

    Ok(panel)
}

/// AOOSTAR-X monitor json configuration file
#[derive(Debug, Serialize, Deserialize)]
pub struct MonitorConfig {
    // _Not used_
    // pub credentials: Option<Credentials>,
    /// Configuration settings.
    pub setup: Setup,
    /// Panels: 1-based index into `panels`
    #[serde(rename = "mianban")]
    pub active_panels: Vec<u32>,
    /// Custom panels / DIY "Do It Yourself",
    #[serde(rename = "diy")]
    pub panels: Vec<Panel>,
    /// Internal index of the currently active panel. 1-based!
    #[serde(skip)]
    active_panel_idx: Option<usize>,
    /// Internal sensor label mapping
    #[serde(skip)]
    sensor_mapping: Option<HashMap<String, String>>,
    /// Internal sensor filter
    #[serde(skip)]
    pub sensor_filter: Option<SensorFilter>,
}

impl MonitorConfig {
    pub fn get_next_active_panel(&mut self) -> Option<&Panel> {
        let mut active_panel_idx = self.active_panel_idx.unwrap_or(0) + 1;
        if active_panel_idx > self.panels.len() {
            active_panel_idx = 1;
        }

        for (index, active) in self
            .active_panels
            .iter()
            .filter(|&active| *active > 0)
            .enumerate()
        {
            if *active > self.panels.len() as u32 {
                warn!("Ignoring invalid active panel {active}");
                continue;
            }
            if index + 1 == active_panel_idx {
                self.active_panel_idx = Some(active_panel_idx);
                return Some(&self.panels[*active as usize - 1]);
            }
        }

        None
    }

    /// Adds a custom panel to the application and maps sensor labels if applicable.
    ///
    /// The panel is marked active and will be returned with [get_next_active_panel] when it is its turn.
    ///
    /// # Arguments
    ///
    /// * `panel` - the Panel to include in the active panels.
    pub fn include_custom_panel(&mut self, mut panel: Panel) {
        if let Some(mapping) = &self.sensor_mapping {
            panel.map_sensor_labels(mapping);
        }
        self.panels.push(panel);
        self.active_panels.push(self.panels.len() as u32);
    }

    /// Apply a sensor label mapping on the included panels.
    ///
    /// The mapping will also be applied on any custom panel added in the future with [include_custom_panel].
    ///
    /// **Attention**: this method may only be called once at startup.
    /// Dynamically changing mappings are not supported, and the original sensor labels are not preserved.
    pub fn set_sensor_mapping(&mut self, mapping: HashMap<String, String>) {
        for panel in self.panels.iter_mut() {
            panel.map_sensor_labels(&mapping);
        }
        self.sensor_mapping = Some(mapping);
    }
}

/// Web-app user login
///
/// Not used, part of AOOSTAR-X json configuration file.
#[derive(Debug, Serialize, Deserialize)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

/// Configuration settings.
///
/// Note: Trimmed down object to include only required fields for `asterctl`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Setup {
    /// Switch time between panels in seconds, interpreted as float and converted to milliseconds. Default: 5
    pub switch_time: Option<String>, // existed as "30" string
    /// Panel redraw interval in seconds. Default: 1
    pub refresh: f32,
    /*
    // The following fields of the AOOSTAR-X json configuration file are NOT used in `asterctl`
    /// Default: true
    pub off_display: bool,
    /// Selection of default panels based on theme / control_params / control_disk_temp ?
    pub theme: i32,
    /// ? Default: true
    pub control_params: bool,
    /// ? Default: true
    pub control_disk_temp: bool,
    /// Default: false
    pub custom_panel: bool,
    /// Language index. Default: 0
    pub language: Language,
    /// Operation mode: performance, power saving, etc.
    pub operation_mode: Option<OperationMode>,
    /// Operation type 1 or 2 (?). Default: 1
    #[serde(rename = "type")]
    pub operation_type: Option<i16>,
    /// Default: 300
    pub disk_update: i32,
    /// Home Assistant URL
    #[serde(deserialize_with = "empty_string_as_none")]
    #[serde(rename = "ha_url")]
    pub ha_url: Option<String>, // "" in JSON ⇒ Option<String>
    /// Home Assistant long-lived access token
    #[serde(deserialize_with = "empty_string_as_none")]
    #[serde(rename = "ha_token")]
    pub ha_token: Option<String>, // "" in JSON ⇒ Option<String>
    */
}

/// Language setting.
///
/// Not used, part of AOOSTAR-X json configuration file.
#[derive(Debug, Copy, Clone, Serialize_repr, Deserialize_repr, PartialEq)]
#[repr(u8)]
#[allow(dead_code)]
pub enum Language {
    Chinese = 0,
    English = 1,
    Japanese = 2,
}

/// Not used, part of AOOSTAR-X json configuration file.
#[derive(Debug, Copy, Clone, Serialize_repr, Deserialize_repr, PartialEq)]
#[repr(i16)]
#[allow(dead_code)]
pub enum OperationMode {
    None = -1,
    HighPerformance = 0,
    Intelligent = 1,
    PowerSaving = 2,
    Custom30W = 3,
    Custom20W = 4,
    Custom10W = 5,
}

#[derive(Debug, Copy, Clone, Serialize_repr, Deserialize_repr, PartialEq)]
#[repr(u8)]
pub enum SensorDirection {
    /// Also used for clockwise in circular/arc progress & rotating pointer/dial indicator
    LeftToRight = 1,
    /// Also used for counter-clockwise in circular/arc & rotating pointer/dial progress indicator
    RightToLeft = 2,
    TopToBottom = 3,
    BottomToTop = 4,
}

/// Custom DIY panel definition
#[derive(Debug, Serialize, Deserialize)]
pub struct Panel {
    /// Custom panel id
    pub id: Option<String>,
    /// Custom panel name
    pub name: Option<String>,
    /*
    // The following fields of the AOOSTAR-X json configuration file are NOT used in `asterctl`
    /// TODO
    pub checked: Option<bool>,
    /// TODO panel type: 5 = built-in? 6 = custom ?
    #[serde(rename = "type")]
    pub panel_type: i32,
     */
    /// Background image filename
    pub img: Option<String>,
    /// Sensors
    pub sensor: Vec<Sensor>,
}

impl Panel {
    pub fn friendly_name(&self) -> String {
        self.name
            .clone()
            .or_else(|| self.id.clone())
            .or_else(|| {
                if let Some(img_file) = &self.img {
                    let img_file = PathBuf::from(img_file);
                    img_file
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "panel".into())
    }

    fn map_sensor_labels(&mut self, mapping: &HashMap<String, String>) {
        for sensor in self.sensor.iter_mut() {
            if let Some(new_label) = mapping.get(&sensor.label) {
                sensor.label = new_label.clone();
            }
        }
    }
}

/// One Data Display Unit
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sensor {
    /// Sensor mode: text, fan, progress, pointer
    pub mode: SensorMode,
    /// Sensor type, _not used_.
    /// - 1 Time / Date Labels
    /// - 2 Windows-specific system info
    /// - 3 Hardware value
    /// - 4 AIDA64 sensor
    /// - 5 HA sensor
    /// - 6 http fetch from url
    /// - 7 system info ?
    /// - 8 lm-sensor ?
    #[serde(rename = "type")]
    pub sensor_type: Option<i32>,
    /// Label name for internal panels.
    pub name: Option<String>,
    /// Label name for custom panels.
    pub item_name: Option<String>,
    /// Label identifier, also used as data source identifier.
    pub label: String,
    /// Sensor value. Ignored: value is used from a sensor source
    #[serde(deserialize_with = "empty_string_as_none")]
    pub value: Option<String>, // "" or numbers, so Option<String>

    /// Image for progress, fan and pointer indicators
    pub min_value: Option<f32>,
    /// Image for progress, fan and pointer indicators
    pub max_value: Option<f32>,

    /// Optional unit text to print after the value
    #[serde(deserialize_with = "empty_string_as_none")]
    pub unit: Option<String>,
    /// Rounded x-position. Custom panel coordinates are stored as float!
    #[serde(deserialize_with = "f32_as_rounded_i32")]
    pub x: i32,
    /// Rounded y-position. Custom panel coordinates are stored as float!
    #[serde(deserialize_with = "f32_as_rounded_i32")]
    pub y: i32,
    /// Used for pointer type
    pub width: Option<u32>,
    /// Used for pointer type
    pub height: Option<u32>,
    /// Sensor graphic orientation
    pub direction: Option<SensorDirection>,

    /// Font name matching font filename without file extension.
    pub font_family: Option<String>,
    /// TODO font size unit: points or pixels?
    pub font_size: Option<i32>,
    /// Font color in `#RRGGBB` notation, or -1 if not set. #ffffff = white, #ff0000 = red
    pub font_color: Option<FontColor>,
    /// _Not (yet) used_
    pub font_weight: Option<FontWeight>,
    pub text_align: Option<TextAlign>,

    /// Number of integer places for the sensor value.
    // -1 ≈ unset ⇒ Option<i32>
    #[serde(deserialize_with = "option_none_if_minus_one")]
    pub integer_digits: Option<i32>,
    /// Number of decimal places for the sensor value.
    // -1 ≈ unset ⇒ Option<i32>
    #[serde(deserialize_with = "option_none_if_minus_one")]
    pub decimal_digits: Option<i32>,
    /// Image for progress, fan and pointer indicators
    #[serde(deserialize_with = "empty_string_as_none")]
    pub pic: Option<String>,

    /// Used for fan & pointer sensors
    pub min_angle: Option<i32>,
    /// Used for fan & pointer sensors
    pub max_angle: Option<i32>,

    /// Pivot x
    #[serde(rename = "xz_x")]
    pub xz_x: Option<i32>,
    /// Pivot y
    #[serde(rename = "xz_y")]
    pub xz_y: Option<i32>,

    /// Colour bands, so the colour carries meaning rather than being decoration.
    ///
    /// Fork addition, not part of the AOOSTAR-X configuration format. The band with the
    /// highest `min` that the value reaches wins. Non-numeric values and values below every
    /// band fall back to `fontColor`.
    #[serde(default)]
    pub thresholds: Option<Vec<Threshold>>,

    /// Substitutions applied to the raw value before rendering.
    ///
    /// Fork addition. Providers emit machine values — Uptime-Kuma reports a monitor's state
    /// as `1` or `0` — which mean nothing on a glanceable panel. This maps them to words.
    /// Colour selection still uses the raw value, so a mapped `DOWN` can still be red.
    #[serde(default)]
    pub value_map: Option<HashMap<String, String>>,

    /// Placeholder text rendered when this sensor has no current value.
    ///
    /// Fork addition, not part of the AOOSTAR-X configuration format. Optional and defaulted,
    /// so vendor configuration files parse unchanged.
    #[serde(default)]
    pub stale_text: Option<String>,
    /// Colour used when rendering a stale placeholder.
    ///
    /// Fork addition, not part of the AOOSTAR-X configuration format. Keep it clear of any
    /// colour used for value thresholds — "no reading" must not look like "critical".
    #[serde(default)]
    pub stale_color: Option<FontColor>,
    /*
    // The following fields of the AOOSTAR-X json configuration file are NOT used in `asterctl`
    /// _Not (yet) used_
    pub text_direction: i32, // layout direction
    /// For type = 6
    pub url: Option<String>,
    /// For type = 6
    pub data: Option<String>,
    /// For type = 6
    pub interval: Option<u32>,
     */
}

/// One colour band of a [`Sensor`]'s threshold list.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Threshold {
    /// Lowest value this colour applies from, inclusive.
    pub min: f32,
    /// Colour to draw with, in `#RRGGBB` notation.
    pub color: FontColor,
}

/// Default placeholder text for a sensor with no current value.
pub const DEFAULT_STALE_TEXT: &str = "--";

/// Default stale colour: mid-grey, deliberately outside any threshold palette.
pub const DEFAULT_STALE_COLOR: Rgb<u8> = Rgb([128, 128, 128]);

impl Sensor {
    /// The value to render when this sensor has no current reading.
    ///
    /// Text sensors get a placeholder string. Graphical modes are driven by a number, so they
    /// fall back to their minimum — an empty bar or a needle at rest. Paired with a `--` text
    /// element that reads as "no data" rather than as a genuine low reading.
    pub fn stale_value(&self) -> String {
        match self.mode {
            SensorMode::Text => self
                .stale_text
                .clone()
                .unwrap_or_else(|| DEFAULT_STALE_TEXT.to_string()),
            _ => self.min_value.unwrap_or_default().to_string(),
        }
    }

    /// The colour this value should be drawn in.
    ///
    /// Falls back to `fontColor` when no thresholds are configured, when the value is not
    /// numeric, or when it sits below every band.
    pub fn colour_for(&self, value: &str) -> FontColor {
        let base = self.font_color.unwrap_or_default();
        let Some(thresholds) = &self.thresholds else {
            return base;
        };
        let Ok(numeric) = value.trim().parse::<f32>() else {
            return base;
        };

        thresholds
            .iter()
            .filter(|t| numeric >= t.min)
            .max_by(|a, b| a.min.total_cmp(&b.min))
            .map(|t| t.color)
            .unwrap_or(base)
    }

    /// The text to render for a raw value, after any configured substitution.
    ///
    /// Matches the trimmed value, then — since we do not control how providers format
    /// numbers — retries on the integer form, so a map keyed on `1` still matches `1.0`.
    pub fn display_value<'a>(&'a self, value: &'a str) -> &'a str {
        let Some(map) = &self.value_map else {
            return value;
        };
        let trimmed = value.trim();

        if let Some(mapped) = map.get(trimmed) {
            return mapped;
        }
        if let Ok(numeric) = trimmed.parse::<f64>()
            && numeric.fract() == 0.0
            && let Some(mapped) = map.get(&format!("{}", numeric.trunc() as i64))
        {
            return mapped;
        }

        value
    }

    /// The configured stale colour, or the default mid-grey.
    pub fn stale_color(&self) -> FontColor {
        self.stale_color
            .unwrap_or_else(|| DEFAULT_STALE_COLOR.into())
    }
}

/// Sensor element type. Name is based on AOOSTAR-X web configuration
#[derive(Debug, Clone, Copy, Serialize_repr, Deserialize_repr, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum SensorMode {
    /// Text element
    Text = 1,
    /// Circular/arc progress indicator
    Fan = 2,
    /// Horizontal or vertical progress indicator
    Progress = 3,
    /// Rotating pointer/dial indicator
    Pointer = 4,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum TimeDateLabel {
    #[serde(rename = "DATE_year")]
    Year,
    #[serde(rename = "DATE_month")]
    Month,
    #[serde(rename = "DATE_day")]
    Day,
    #[serde(rename = "DATE_hour")]
    Hour,
    #[serde(rename = "DATE_minute")]
    Minute,
    #[serde(rename = "DATE_second")]
    Second,
    #[serde(rename = "DATE_m_d_h_m_1")]
    MDHM1,
    #[serde(rename = "DATE_m_d_h_m_2")]
    MDHM2,
    #[serde(rename = "DATE_m_d_1")]
    MD1,
    #[serde(rename = "DATE_m_d_2")]
    MD2,
    #[serde(rename = "DATE_y_m_d_1")]
    YMD1,
    #[serde(rename = "DATE_y_m_d_2")]
    YMD2,
    #[serde(rename = "DATE_y_m_d_3")]
    YMD3,
    #[serde(rename = "DATE_y_m_d_4")]
    YMD4,
    #[serde(rename = "DATE_h_m_s_1")]
    HMS1,
    #[serde(rename = "DATE_h_m_s_2")]
    HMS2,
    #[serde(rename = "DATE_h_m_s_3")]
    HMS3,
    #[serde(rename = "DATE_h_m_1")]
    HM1,
    #[serde(rename = "DATE_h_m_2")]
    HM2,
    #[serde(rename = "DATE_h_m_3")]
    HM3,
}

#[derive(Debug, Copy, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FontWeight {
    #[default]
    Normal,
    Bold,
}

#[derive(Debug, Copy, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

fn option_none_if_minus_one<'de, D>(deserializer: D) -> Result<Option<i32>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<i32>::deserialize(deserializer)? {
        Some(-1) | None => Ok(None),
        Some(other) => Ok(Some(other)),
    }
}

/// Special font color type since it is represented either as numeric -1 or as a string :-(
///
/// A good serde programming exercise...
#[derive(Debug, Clone, Copy)]
pub struct FontColor(Rgb<u8>);

impl Default for FontColor {
    fn default() -> Self {
        FontColor(Rgb::white())
    }
}

impl Deref for FontColor {
    type Target = Rgb<u8>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl TryFrom<&str> for FontColor {
    type Error = ParseIntError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value.len() != 7 || !value.starts_with('#') {
            warn!("Invalid font color: {value}");
            Ok(FontColor::default())
        } else {
            Ok(FontColor(Rgb([
                u8::from_str_radix(&value[1..3], 16)?,
                u8::from_str_radix(&value[3..5], 16)?,
                u8::from_str_radix(&value[5..7], 16)?,
            ])))
        }
    }
}

impl From<Rgb<u8>> for FontColor {
    fn from(value: Rgb<u8>) -> Self {
        FontColor(value)
    }
}

impl From<FontColor> for Rgb<u8> {
    fn from(val: FontColor) -> Self {
        val.0
    }
}

impl From<FontColor> for Rgba<u8> {
    fn from(val: FontColor) -> Self {
        Rgba([val.0[0], val.0[1], val.0[2], 255])
    }
}

impl Serialize for FontColor {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        format!("#{:02x}{:02x}{:02x}", self.0[0], self.0[1], self.0[2]).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for FontColor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct MyVisitor;

        impl<'de> Visitor<'de> for MyVisitor {
            type Value = FontColor;

            fn expecting(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt.write_str("integer or string")
            }

            fn visit_i64<E>(self, val: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match val {
                    -1 => Ok(FontColor::default()),
                    _ => Err(E::custom("invalid integer value, expected -1")),
                }
            }

            fn visit_str<E>(self, val: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if val.trim().is_empty() {
                    return Ok(FontColor::default());
                }
                match val.parse::<i32>() {
                    Ok(val) => self.visit_i32(val),
                    Err(_) => val
                        .try_into()
                        .map_err(|e| E::custom(format!("invalid font color value: {e}"))),
                }
            }
        }

        deserializer.deserialize_any(MyVisitor)
    }
}

fn empty_string_as_none<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let option = Option::<String>::deserialize(deserializer)?;
    Ok(option.and_then(|s| if s.trim().is_empty() { None } else { Some(s) }))
}

fn f32_as_rounded_i32<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: Deserializer<'de>,
{
    let rounded = f32::deserialize(deserializer).map(f32::round)?;
    Ok(rounded as i32)
}

#[cfg(test)]
mod value_presentation_tests {
    use super::*;

    // r##"..."## throughout: these fixtures contain "# from the colour literals, which
    // would terminate a single-hash raw string.
    fn sensor_json(extra: &str) -> Sensor {
        // Fields using a custom deserializer must be present: serde does not imply a
        // default for them, so a minimal fixture has to spell them out.
        let json = format!(
            r##"{{"mode":1,"label":"t","x":0,"y":0,"value":"","unit":"","pic":"",
                  "integerDigits":-1,"decimalDigits":-1,"fontColor":"#ffffff"{extra}}}"##
        );
        serde_json::from_str(&json).expect("sensor should parse")
    }

    const BANDS: &str = r##","thresholds":[
        {"min":0,"color":"#7ee787"},
        {"min":70,"color":"#ffb454"},
        {"min":85,"color":"#ff6b6b"}]"##;

    fn rgb(s: &Sensor, value: &str) -> [u8; 3] {
        let c: Rgb<u8> = s.colour_for(value).into();
        c.0
    }

    #[test]
    fn without_thresholds_the_base_colour_is_used() {
        assert_eq!(rgb(&sensor_json(""), "99"), [255, 255, 255]);
    }

    #[test]
    fn the_highest_band_the_value_reaches_wins() {
        let s = sensor_json(BANDS);
        assert_eq!(
            rgb(&s, "10"),
            [0x7e, 0xe7, 0x87],
            "low sits in the first band"
        );
        assert_eq!(
            rgb(&s, "75"),
            [0xff, 0xb4, 0x54],
            "mid crosses into the second"
        );
        assert_eq!(rgb(&s, "92"), [0xff, 0x6b, 0x6b], "high reaches the third");
    }

    #[test]
    fn a_band_boundary_is_inclusive() {
        assert_eq!(rgb(&sensor_json(BANDS), "70"), [0xff, 0xb4, 0x54]);
    }

    #[test]
    fn values_below_every_band_fall_back() {
        let s = sensor_json(r##","thresholds":[{"min":50,"color":"#ff0000"}]"##);
        assert_eq!(rgb(&s, "10"), [255, 255, 255]);
    }

    #[test]
    fn non_numeric_values_fall_back() {
        assert_eq!(rgb(&sensor_json(BANDS), "192.0.2.24"), [255, 255, 255]);
    }

    #[test]
    fn negative_values_are_handled() {
        let s = sensor_json(r##","thresholds":[{"min":-10,"color":"#00ff00"}]"##);
        assert_eq!(rgb(&s, "-5"), [0, 255, 0]);
    }

    #[test]
    fn without_a_map_the_value_passes_through() {
        assert_eq!(sensor_json("").display_value("1"), "1");
    }

    #[test]
    fn mapped_values_are_substituted() {
        let s = sensor_json(r##","valueMap":{"1":"UP","0":"DOWN"}"##);
        assert_eq!(s.display_value("1"), "UP");
        assert_eq!(s.display_value("0"), "DOWN");
    }

    /// Providers format numbers as they please; a map keyed on 1 must still match 1.0.
    #[test]
    fn a_float_formatted_integer_still_matches() {
        let s = sensor_json(r##","valueMap":{"1":"UP"}"##);
        assert_eq!(s.display_value("1.0"), "UP");
        assert_eq!(s.display_value(" 1 "), "UP");
    }

    #[test]
    fn unmapped_values_pass_through_untouched() {
        let s = sensor_json(r##","valueMap":{"1":"UP"}"##);
        assert_eq!(s.display_value("7"), "7");
        assert_eq!(s.display_value("hello"), "hello");
    }

    /// The point of separating the two: a mapped word still colours from the number.
    #[test]
    fn colour_comes_from_the_raw_value_not_the_mapped_text() {
        let s = sensor_json(
            r##","valueMap":{"0":"DOWN","1":"UP"},"thresholds":[{"min":0,"color":"#ff0000"},{"min":1,"color":"#00ff00"}]"##,
        );
        assert_eq!(s.display_value("0"), "DOWN");
        assert_eq!(rgb(&s, "0"), [255, 0, 0], "DOWN must still be red");
        assert_eq!(rgb(&s, "1"), [0, 255, 0], "UP must still be green");
    }

    #[test]
    fn vendor_configuration_without_the_new_fields_still_parses() {
        let s = sensor_json("");
        assert!(s.thresholds.is_none());
        assert!(s.value_map.is_none());
    }
}
