//! User configuration (`config.toml`).
//!
//! The file lives at `<config dir>/rigtop/config.toml`
//! (`~/.config/rigtop/config.toml` on Linux, `%APPDATA%\rigtop\config.toml`
//! on Windows). A missing file means "all defaults".
//!
//! ```toml
//! [sensors]
//! # Names for sensor channels, keyed by the stable channel key shown in the UI.
//! labels = { "nct6686:nct6683.2592/fan1" = "CPU fan", "nct6686:nct6683.2592/temp1" = "VRM" }
//! # Channels to hide (e.g. unconnected thermistors reading -40 °C).
//! hidden = ["nct6686:nct6683.2592/temp4"]
//!
//! [smart]
//! # Directory with smartctl JSON files written by the root timer.
//! dir = "/var/lib/rigtop/smart"
//! ```
//!
//! Labels are applied by front-ends rather than by data sources, so that a
//! client monitoring remote hosts uses its own local configuration.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::snapshot::{Fan, Temperature};

/// Default location of the SMART result files.
pub const DEFAULT_SMART_DIR: &str = "/var/lib/rigtop/smart";

/// Errors while loading or saving the configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file could not be read or written.
    #[error("cannot access {path}: {source}")]
    Io {
        /// File that failed.
        path: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
    /// The file is not valid TOML or has unexpected fields.
    #[error("invalid config {path}: {source}")]
    Parse {
        /// File that failed.
        path: PathBuf,
        /// Underlying error.
        source: toml::de::Error,
    },
    /// The configuration could not be serialized.
    #[error("cannot serialize config: {0}")]
    Serialize(#[from] toml::ser::Error),
}

/// Top-level configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Sensor naming and visibility.
    pub sensors: SensorConfig,
    /// SMART result files.
    pub smart: SmartConfig,
}

/// Sensor naming and visibility.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SensorConfig {
    /// User-defined names keyed by channel key (`"<chip key>/fan1"`, `"<chip key>/temp3"`).
    /// A `BTreeMap` keeps the saved file in a stable order.
    pub labels: BTreeMap<String, String>,
    /// Channel keys to hide.
    pub hidden: Vec<String>,
}

/// Where to find SMART results.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SmartConfig {
    /// Directory containing one smartctl JSON file per device.
    pub dir: PathBuf,
}

impl Default for SmartConfig {
    fn default() -> Self {
        Self {
            dir: PathBuf::from(DEFAULT_SMART_DIR),
        }
    }
}

impl Config {
    /// Default path of the configuration file, if the platform has a config directory.
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("rigtop").join("config.toml"))
    }

    /// Loads the configuration from `path`. A missing file yields the defaults.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(ConfigError::Io {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_owned(),
            source,
        })
    }

    /// Loads from [`Config::default_path`], or returns the defaults when the
    /// platform has no config directory.
    pub fn load_default() -> Result<Self, ConfigError> {
        match Self::default_path() {
            Some(path) => Self::load(&path),
            None => Ok(Self::default()),
        }
    }

    /// Writes the configuration to `path`, creating parent directories.
    ///
    /// The file is written to a temporary sibling and renamed into place, so
    /// a crash never leaves a truncated config behind.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let text = toml::to_string_pretty(self)?;
        let io_err = |source| ConfigError::Io {
            path: path.to_owned(),
            source,
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_err)?;
        }
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, text).map_err(io_err)?;
        fs::rename(&tmp, path).map_err(io_err)
    }

    /// Display name of a fan: the user-defined label, or the driver/generic label.
    pub fn fan_label<'a>(&'a self, fan: &'a Fan) -> &'a str {
        self.label_for(&fan.key).unwrap_or(&fan.label)
    }

    /// Display name of a temperature channel.
    pub fn temperature_label<'a>(&'a self, temperature: &'a Temperature) -> &'a str {
        self.label_for(&temperature.key)
            .unwrap_or(&temperature.label)
    }

    /// User-defined label for a channel key, if any.
    pub fn label_for(&self, key: &str) -> Option<&str> {
        self.sensors.labels.get(key).map(String::as_str)
    }

    /// Sets (or, with an empty name, removes) the label of a channel.
    pub fn set_label(&mut self, key: &str, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            self.sensors.labels.remove(key);
        } else {
            self.sensors.labels.insert(key.to_owned(), name.to_owned());
        }
    }

    /// Whether the user chose to hide a channel.
    pub fn is_hidden(&self, key: &str) -> bool {
        self.sensors.hidden.iter().any(|k| k == key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_gives_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let config = Config::load(&tmp.path().join("none.toml")).unwrap();
        assert_eq!(config, Config::default());
        assert_eq!(config.smart.dir, PathBuf::from(DEFAULT_SMART_DIR));
    }

    #[test]
    fn parses_labels_hidden_and_smart_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(
            &path,
            r#"
[sensors]
labels = { "nct6686:nct6683.2592/fan1" = "CPU fan" }
hidden = ["nct6686:nct6683.2592/temp4"]

[smart]
dir = "/tmp/smart"
"#,
        )
        .unwrap();
        let config = Config::load(&path).unwrap();
        let fan = Fan {
            key: "nct6686:nct6683.2592/fan1".to_owned(),
            label: "fan1".to_owned(),
            rpm: 900,
        };
        let other = Fan {
            key: "nct6686:nct6683.2592/fan2".to_owned(),
            label: "fan2".to_owned(),
            rpm: 0,
        };
        assert_eq!(config.fan_label(&fan), "CPU fan");
        // Unnamed channels fall back to the generic label.
        assert_eq!(config.fan_label(&other), "fan2");
        assert!(config.is_hidden("nct6686:nct6683.2592/temp4"));
        assert_eq!(config.smart.dir, PathBuf::from("/tmp/smart"));
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        // A typo such as "label" instead of "labels" must not be silently ignored.
        fs::write(&path, "[sensors]\nlabel = {}\n").unwrap();
        assert!(matches!(
            Config::load(&path),
            Err(ConfigError::Parse { .. })
        ));
    }

    #[test]
    fn save_and_reload_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested/rigtop/config.toml");
        let mut config = Config::default();
        config.set_label("k10temp:0000:00:18.3/temp3", " CCD1 ");
        config.set_label("nct6686:nct6683.2592/fan1", "Rear fan");
        config.set_label("nct6686:nct6683.2592/fan1", "");
        config.save(&path).unwrap();

        let back = Config::load(&path).unwrap();
        assert_eq!(back.label_for("k10temp:0000:00:18.3/temp3"), Some("CCD1"));
        // An empty name removes the label.
        assert_eq!(back.label_for("nct6686:nct6683.2592/fan1"), None);
        assert!(!path.with_extension("toml.tmp").exists());
    }
}
