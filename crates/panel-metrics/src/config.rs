// SPDX-License-Identifier: MIT OR Apache-2.0

//! Configuration, and the loading of credentials from outside it.
//!
//! # Credentials are never values in this file
//!
//! Every source names a *path* to read its secret from, never the secret itself. A key in a
//! config file is a key in a backup, in a paste, and eventually in a repository — this
//! project has already leaked one that way. Paths keep the config safe to commit, diff and
//! share.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn default_listen() -> String {
    "0.0.0.0:9101".to_string()
}

fn default_refresh() -> u64 {
    30
}

fn default_timeout() -> u64 {
    10
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Address to serve `/metrics` on.
    #[serde(default = "default_listen")]
    pub listen: String,

    /// How often every source is scraped, in seconds.
    ///
    /// Sources are polled on this schedule and the result is cached. Scraping on request
    /// instead would let a single consumer's poll rate drive load on four upstream APIs,
    /// and OPNsense in particular does not deserve that.
    #[serde(default = "default_refresh")]
    pub refresh_seconds: u64,

    #[serde(default)]
    pub truenas: Option<TrueNas>,
    #[serde(default)]
    pub kuma: Option<Kuma>,
    #[serde(default)]
    pub opnsense: Option<OpnSense>,
    #[serde(default)]
    pub hass: Option<Hass>,
}

/// Shared shape for every source: where it is, how long to wait, whether its certificate
/// can be trusted.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub url: String,
    /// File containing the credential. Never the credential itself.
    pub token_file: PathBuf,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    /// Accept an invalid or expired TLS certificate.
    ///
    /// Needed for appliances using their own self-signed certificate. Scoped per source so
    /// trusting one box does not quietly trust the rest.
    #[serde(default)]
    pub accept_invalid_cert: bool,
}

impl Endpoint {
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_seconds)
    }

    /// Read the credential, trimming the trailing newline every editor adds.
    pub fn secret(&self) -> Result<String> {
        read_secret(&self.token_file)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrueNas {
    #[serde(flatten)]
    pub endpoint: Endpoint,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Kuma {
    #[serde(flatten)]
    pub endpoint: Endpoint,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpnSense {
    #[serde(flatten)]
    pub endpoint: Endpoint,
    /// File containing the API key, used as the basic-auth username.
    ///
    /// OPNsense splits its credential in two. Both halves live outside the config.
    pub key_file: PathBuf,
}

impl OpnSense {
    pub fn key(&self) -> Result<String> {
        read_secret(&self.key_file)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hass {
    #[serde(flatten)]
    pub endpoint: Endpoint,
    /// Entity IDs to publish, for example `sensor.hypervolt_session_energy`.
    ///
    /// Explicit rather than a bulk fetch of everything: entity IDs are not predictable —
    /// recon found `hypervolt_charge_power` did not exist while `hypervolt_session_energy`
    /// did — so naming them makes a rename visible as a missing metric rather than as a
    /// quietly absent row.
    #[serde(default)]
    pub entities: Vec<String>,
}

fn read_secret(path: &Path) -> Result<String> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("Failed to read credential file {}", path.display()))?;
    let secret = contents.trim_end_matches(['\n', '\r']).to_string();
    if secret.is_empty() {
        anyhow::bail!("Credential file {} is empty", path.display());
    }
    Ok(secret)
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config {}", path.display()))?;
        let config: Config = toml::from_str(&text)
            .with_context(|| format!("Failed to parse config {}", path.display()))?;

        if config.refresh_seconds == 0 {
            anyhow::bail!("refresh_seconds must be greater than zero");
        }
        Ok(config)
    }

    pub fn refresh(&self) -> Duration {
        Duration::from_secs(self.refresh_seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(name: &str, contents: &str) -> PathBuf {
        let path = std::env::temp_dir().join(name);
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        path
    }

    #[test]
    fn a_minimal_config_parses_with_defaults() {
        let path = write_temp("pm-minimal.toml", "");
        let cfg = Config::load(&path).unwrap();

        assert_eq!(cfg.listen, "0.0.0.0:9101");
        assert_eq!(cfg.refresh_seconds, 30);
        assert!(cfg.truenas.is_none());
    }

    #[test]
    fn a_source_parses_with_its_endpoint_flattened() {
        let path = write_temp(
            "pm-truenas.toml",
            r#"
            [truenas]
            url = "https://192.0.2.24"
            token_file = "/etc/panel-metrics/truenas.key"
            accept_invalid_cert = true
            "#,
        );
        let cfg = Config::load(&path).unwrap();
        let truenas = cfg.truenas.expect("should parse");

        assert_eq!(truenas.endpoint.url, "https://192.0.2.24");
        assert!(truenas.endpoint.accept_invalid_cert);
        assert_eq!(truenas.endpoint.timeout_seconds, 10, "default applies");
    }

    /// A typo in a config file should fail loudly at startup, not silently disable a source.
    #[test]
    fn an_unknown_field_is_rejected() {
        let path = write_temp("pm-typo.toml", "listne = \"0.0.0.0:1\"\n");
        assert!(Config::load(&path).is_err());
    }

    #[test]
    fn a_zero_refresh_is_rejected() {
        let path = write_temp("pm-zero.toml", "refresh_seconds = 0\n");
        let err = Config::load(&path).unwrap_err().to_string();
        assert!(err.contains("greater than zero"), "got: {err}");
    }

    #[test]
    fn a_secret_loses_its_trailing_newline() {
        let path = write_temp("pm-secret.key", "abc123\n");
        assert_eq!(read_secret(&path).unwrap(), "abc123");
    }

    #[test]
    fn a_secret_keeps_internal_whitespace() {
        let path = write_temp("pm-secret2.key", "abc 123\n");
        assert_eq!(read_secret(&path).unwrap(), "abc 123");
    }

    /// An empty credential file would otherwise authenticate as nobody and fail confusingly
    /// downstream as a 401 rather than as a configuration problem.
    #[test]
    fn an_empty_secret_is_an_error() {
        let path = write_temp("pm-empty.key", "\n");
        assert!(read_secret(&path).is_err());
    }

    #[test]
    fn a_missing_secret_names_the_file() {
        let err = read_secret(Path::new("/nope/missing.key"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing.key"), "got: {err}");
    }
}
