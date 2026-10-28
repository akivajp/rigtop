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
//!
//! [btrfs]
//! # Directory with btrfs scrub status files copied by the root timer.
//! scrub_dir = "/var/lib/rigtop/btrfs"
//!
//! [alerts]
//! # When readings become alerts (0 turns one off).
//! filesystem_warning_percent = 90
//! filesystem_critical_percent = 97
//! cpu_warning_celsius = 96    # hottest CPU reading
//! gpu_warning_celsius = 85
//! drive_warning_celsius = 70  # SMART or NVMe sensor
//! ignored_units = []          # failed systemd units not to warn about
//!
//! [notifications]
//! # rigtop-gui: desktop notifications for new alerts ("critical", "warning" or "off").
//! level = "critical"
//!
//! [updates]
//! # Look for new releases on github.com (at startup and every interval:
//! # "30m", "6h", "1d", "1h30m", ...; at least a minute).
//! check = true
//! interval = "1h"
//! notify = true   # rigtop-gui: desktop notification for a new release
//!
//! [lhm]
//! # Windows: LibreHardwareMonitor's web server ("" to disable).
//! url = "http://127.0.0.1:8085/data.json"
//!
//! [overview]
//! # Sections of each host card, most important first.
//! sections = ["cpu", "gpu", "memory", "processes", "network", "storage"]
//! include_local = true
//! processes = 10
//!
//! [[hosts]]
//! name = "zefat"
//!
//! [[hosts]]
//! name = "nas"
//! ssh = "admin@nas.local"
//! command = "/usr/local/bin/rigtop --agent"
//! sections = ["storage", "cpu", "network"]
//!
//! [[widgets]]
//! name = "Clock"
//! builtin = "clock"
//!
//! [[widgets]]
//! name = "Rates"
//! command = "~/bin/rates.sh"   # prints text or JSON rows, see `widget`
//! interval_secs = 60
//!
//! ```
//!
//! Labels are applied by front-ends rather than by data sources, so that a
//! client monitoring remote hosts uses its own local configuration.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

use crate::snapshot::{Fan, Temperature};

/// Default location of the SMART result files.
pub const DEFAULT_SMART_DIR: &str = "/var/lib/rigtop/smart";
/// Default location of the copied btrfs scrub status files.
pub const DEFAULT_SCRUB_DIR: &str = "/var/lib/rigtop/btrfs";
/// Default address of LibreHardwareMonitor's web server (Windows sensors).
pub const DEFAULT_LHM_URL: &str = "http://127.0.0.1:8085/data.json";

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
    /// The file is not valid TOML, so it cannot be edited in place.
    #[error("cannot edit {path}: {source}")]
    Edit {
        /// File that failed.
        path: PathBuf,
        /// Underlying error.
        source: toml_edit::TomlError,
    },
}

/// Top-level configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Sensor naming and visibility.
    pub sensors: SensorConfig,
    /// SMART result files.
    pub smart: SmartConfig,
    /// btrfs scrub status files.
    pub btrfs: BtrfsConfig,
    /// LibreHardwareMonitor's web server, the source of temperatures, fans
    /// and CPU power on Windows.
    pub lhm: LhmConfig,
    /// The multi-host overview.
    pub overview: OverviewConfig,
    /// When a reading becomes an alert.
    pub alerts: AlertsConfig,
    /// Desktop notifications for new alerts (GUI).
    pub notifications: NotificationsConfig,
    /// Looking for new rigtop releases.
    pub updates: UpdatesConfig,
    /// Remote hosts monitored over SSH (`[[hosts]]` tables).
    pub hosts: Vec<HostConfig>,
    /// Extra panels beside the monitor (`[[widgets]]` tables).
    pub widgets: Vec<WidgetConfig>,
}

/// Widgets drawn by rigtop itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuiltinWidget {
    /// The date and a large clock.
    Clock,
}

/// One widget: a built-in one, or a command whose output is shown (see
/// [`crate::widget`] for the output format).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WidgetConfig {
    /// Title of the widget.
    pub name: String,
    /// Shell command printing the content.
    pub command: Option<String>,
    /// Or a built-in widget.
    pub builtin: Option<BuiltinWidget>,
    /// Seconds between runs of the command.
    pub interval_secs: u64,
    /// Seconds after which a run is killed.
    pub timeout_secs: u64,
}

impl Default for WidgetConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            command: None,
            builtin: None,
            interval_secs: 5,
            timeout_secs: 10,
        }
    }
}

/// A block of information on a host card in the overview.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OverviewSection {
    Cpu,
    Gpu,
    Memory,
    Processes,
    Network,
    Storage,
}

impl OverviewSection {
    /// All sections in the default priority order.
    pub const ALL: [OverviewSection; 6] = [
        OverviewSection::Cpu,
        OverviewSection::Gpu,
        OverviewSection::Memory,
        OverviewSection::Processes,
        OverviewSection::Network,
        OverviewSection::Storage,
    ];

    /// Name shown in menus.
    pub fn label(self) -> &'static str {
        match self {
            OverviewSection::Cpu => "CPU",
            OverviewSection::Gpu => "GPU",
            OverviewSection::Memory => "Memory",
            OverviewSection::Processes => "Processes",
            OverviewSection::Network => "Network",
            OverviewSection::Storage => "Storage",
        }
    }
}

/// Settings of the multi-host overview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OverviewConfig {
    /// Sections shown on each host card, most important first. Sections are
    /// dropped from the end when a card is too small; leaving one out of
    /// the list hides it.
    pub sections: Vec<OverviewSection>,
    /// Whether this machine also gets a card (it always has its own tab).
    pub include_local: bool,
    /// Processes listed per card. Hosts not shown in detail only send this
    /// many, to keep SSH traffic small.
    pub processes: usize,
}

impl Default for OverviewConfig {
    fn default() -> Self {
        Self {
            sections: OverviewSection::ALL.to_vec(),
            include_local: true,
            processes: 10,
        }
    }
}

/// A remote host to monitor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostConfig {
    /// Name shown in tabs and on the overview card.
    pub name: String,
    /// SSH destination (a `~/.ssh/config` host, `user@host`, ...);
    /// defaults to `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<String>,
    /// Command that starts the agent; defaults to `rigtop --agent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Overview sections for this host, overriding `[overview] sections`
    /// (e.g. storage first for a file server).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sections: Option<Vec<OverviewSection>>,
}

impl HostConfig {
    /// A host given only by its SSH destination (e.g. on the command line).
    pub fn from_destination(destination: &str) -> Self {
        Self {
            name: destination.to_owned(),
            ssh: None,
            command: None,
            sections: None,
        }
    }

    /// The SSH destination.
    pub fn destination(&self) -> &str {
        self.ssh.as_deref().unwrap_or(&self.name)
    }
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

/// Which new alerts the GUI announces with a desktop notification.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotifyLevel {
    /// No notifications.
    Off,
    /// Warnings and critical alerts.
    Warning,
    /// Critical alerts only (data at risk).
    #[default]
    Critical,
}

/// Thresholds that turn readings into alerts (overview cards, status dots
/// and notifications). 0 turns an alert off. Applied by the front-end, so
/// remote hosts are judged by the viewer's settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AlertsConfig {
    /// Filesystem usage (%) for a warning.
    pub filesystem_warning_percent: f64,
    /// Filesystem usage (%) for a critical alert.
    pub filesystem_critical_percent: f64,
    /// Hottest CPU reading (°C) for a warning. Desktop Ryzen CPUs run up to
    /// 95 °C under full load by design.
    pub cpu_warning_celsius: f32,
    /// GPU temperature (°C) for a warning.
    pub gpu_warning_celsius: f32,
    /// Drive temperature (°C, from SMART or the NVMe sensor) for a warning.
    pub drive_warning_celsius: f32,
    /// systemd units whose failure is not worth a warning (e.g. a
    /// `systemd-networkd-wait-online.service` that fails on every boot).
    pub ignored_units: Vec<String>,
}

impl Default for AlertsConfig {
    fn default() -> Self {
        Self {
            filesystem_warning_percent: 90.0,
            filesystem_critical_percent: 97.0,
            cpu_warning_celsius: 96.0,
            gpu_warning_celsius: 85.0,
            drive_warning_celsius: 70.0,
            ignored_units: Vec::new(),
        }
    }
}

/// Desktop notification settings.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NotificationsConfig {
    pub level: NotifyLevel,
}

/// Looking for new rigtop releases on GitHub.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UpdatesConfig {
    /// Look for new releases (contacts github.com); `false` never does.
    pub check: bool,
    /// Time between checks (also checked at startup), written like "1h",
    /// "30m", "1d" or "1h30m".
    #[serde(with = "duration_text")]
    pub interval: Duration,
    /// Announce a new release with a desktop notification (GUI), besides
    /// the note in the title.
    pub notify: bool,
}

impl Default for UpdatesConfig {
    fn default() -> Self {
        Self {
            check: true,
            interval: Duration::from_secs(3600),
            notify: true,
        }
    }
}

impl UpdatesConfig {
    /// Time between checks; at least a minute, whatever the file says.
    pub fn check_interval(&self) -> Duration {
        self.interval.max(Duration::from_secs(60))
    }
}

/// Durations written as text: a number and a unit (`s`, `m`, `h` or `d`),
/// possibly several, as in "1h30m".
mod duration_text {
    use std::time::Duration;

    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serializer};

    const UNITS: [(char, u64); 4] = [('d', 86_400), ('h', 3600), ('m', 60), ('s', 1)];

    pub fn serialize<S: Serializer>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format(*duration))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Duration, D::Error> {
        let text = String::deserialize(deserializer)?;
        parse(&text).ok_or_else(|| {
            D::Error::custom(format!(
                "invalid duration {text:?}: write a number and s, m, h or d, \
                 e.g. \"30m\", \"6h\" or \"1h30m\""
            ))
        })
    }

    /// "1h30m" -> 90 minutes; `None` for anything else.
    pub fn parse(text: &str) -> Option<Duration> {
        let text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        if text.is_empty() {
            return None;
        }
        let mut total = 0u64;
        let mut number = String::new();
        for c in text.chars() {
            if c.is_ascii_digit() {
                number.push(c);
                continue;
            }
            let (_, secs) = UNITS
                .iter()
                .find(|(unit, _)| *unit == c.to_ascii_lowercase())?;
            let value: u64 = number.parse().ok()?;
            total = total.checked_add(value.checked_mul(*secs)?)?;
            number.clear();
        }
        // A number without a unit at the end is ambiguous.
        number.is_empty().then(|| Duration::from_secs(total))
    }

    /// 90 minutes -> "1h30m"; whole seconds only.
    pub fn format(duration: Duration) -> String {
        let mut secs = duration.as_secs();
        if secs == 0 {
            return "0s".to_owned();
        }
        let mut text = String::new();
        for (unit, size) in UNITS {
            if secs >= size {
                text.push_str(&format!("{}{unit}", secs / size));
                secs %= size;
            }
        }
        text
    }
}

/// Where to read Windows sensors from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LhmConfig {
    /// `data.json` of LibreHardwareMonitor's "Remote Web Server" (plain
    /// HTTP without authentication); empty to disable.
    pub url: String,
}

impl Default for LhmConfig {
    fn default() -> Self {
        Self {
            url: DEFAULT_LHM_URL.to_owned(),
        }
    }
}

/// Where to find btrfs scrub results.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BtrfsConfig {
    /// Directory with world-readable copies of `scrub.status.<fsid>`.
    pub scrub_dir: PathBuf,
}

impl Default for BtrfsConfig {
    fn default() -> Self {
        Self {
            scrub_dir: PathBuf::from(DEFAULT_SCRUB_DIR),
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

    /// Replaces the `[[hosts]]` of the configuration file at `path` with
    /// `hosts`, keeping everything else in the file (other settings,
    /// comments, formatting) untouched. Creates the file if needed.
    ///
    /// Used by the GUI's host editor. The edited file is checked to load as
    /// a valid configuration before it replaces the old one.
    pub fn save_hosts(path: &Path, hosts: &[HostConfig]) -> Result<(), ConfigError> {
        edit_file(path, |document| {
            if hosts.is_empty() {
                document.remove("hosts");
                return Ok(());
            }
            let mut tables = toml_edit::ArrayOfTables::new();
            for host in hosts {
                let mut table = toml_edit::Table::new();
                table["name"] = toml_edit::value(host.name.as_str());
                if let Some(ssh) = &host.ssh {
                    table["ssh"] = toml_edit::value(ssh.as_str());
                }
                if let Some(command) = &host.command {
                    table["command"] = toml_edit::value(command.as_str());
                }
                if let Some(sections) = &host.sections {
                    let mut array = toml_edit::Array::new();
                    for section in sections {
                        // Serialized names match the serde representation.
                        let name = toml::Value::try_from(section)?;
                        if let Some(name) = name.as_str() {
                            array.push(name);
                        }
                    }
                    table["sections"] = toml_edit::value(array);
                }
                tables.push(table);
            }
            document["hosts"] = toml_edit::Item::ArrayOfTables(tables);
            Ok(())
        })
    }

    /// Replaces the sensor names and hidden sensors (`[sensors]`) of the
    /// configuration file at `path`, keeping the rest of the file as it is,
    /// like [`Config::save_hosts`]. Names stay an inline table if they were
    /// one.
    ///
    /// Used by the GUI's sensor editor.
    pub fn save_sensors(path: &Path, sensors: &SensorConfig) -> Result<(), ConfigError> {
        edit_file(path, |document| {
            if !document.contains_key("sensors") {
                if sensors.labels.is_empty() && sensors.hidden.is_empty() {
                    return Ok(());
                }
                document["sensors"] = toml_edit::table();
            }
            let Some(table) = document["sensors"].as_table_like_mut() else {
                // Not a table: loading would have failed already.
                return Ok(());
            };
            if sensors.hidden.is_empty() {
                table.remove("hidden");
            } else {
                let hidden: toml_edit::Array = sensors.hidden.iter().map(String::as_str).collect();
                table.insert("hidden", toml_edit::value(hidden));
            }
            let inline = table.get("labels").is_some_and(|l| l.is_inline_table());
            if sensors.labels.is_empty() {
                table.remove("labels");
            } else if inline {
                let labels: toml_edit::InlineTable = sensors
                    .labels
                    .iter()
                    .map(|(k, v)| (k.as_str(), toml_edit::Value::from(v.as_str())))
                    .collect();
                table.insert("labels", toml_edit::value(labels));
            } else {
                let mut labels = toml_edit::Table::new();
                for (key, name) in &sensors.labels {
                    labels[key.as_str()] = toml_edit::value(name.as_str());
                }
                table.insert("labels", toml_edit::Item::Table(labels));
            }
            if table.is_empty() {
                document.remove("sensors");
            }
            Ok(())
        })
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

/// Reads the configuration file at `path` (or an empty one), lets `edit`
/// change it with comments and formatting kept, and writes it back if it
/// still loads as a configuration (through a temporary file, so a crash
/// never leaves a truncated file behind).
fn edit_file(
    path: &Path,
    edit: impl FnOnce(&mut toml_edit::DocumentMut) -> Result<(), ConfigError>,
) -> Result<(), ConfigError> {
    let io_err = |source| ConfigError::Io {
        path: path.to_owned(),
        source,
    };
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(io_err(err)),
    };
    let mut document: toml_edit::DocumentMut =
        text.parse().map_err(|source| ConfigError::Edit {
            path: path.to_owned(),
            source,
        })?;
    edit(&mut document)?;
    let edited = document.to_string();
    // Never write a file that rigtop itself could not load afterwards.
    toml::from_str::<Config>(&edited).map_err(|source| ConfigError::Parse {
        path: path.to_owned(),
        source,
    })?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io_err)?;
    }
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, edited).map_err(io_err)?;
    fs::rename(&tmp, path).map_err(io_err)
}

/// How often [`ConfigWatcher::poll`] looks at the file.
const WATCH_INTERVAL: Duration = Duration::from_secs(1);

/// Notices when the configuration file changes, so a running rigtop can
/// apply the edits without a restart.
pub struct ConfigWatcher {
    path: PathBuf,
    /// Modification time and size when last seen (`None`: no file).
    stamp: Option<(Option<SystemTime>, u64)>,
    last_check: Instant,
    interval: Duration,
}

impl ConfigWatcher {
    /// Watches `path`, taking its current state as already loaded.
    pub fn new(path: PathBuf) -> Self {
        Self {
            stamp: file_stamp(&path),
            path,
            last_check: Instant::now(),
            interval: WATCH_INTERVAL,
        }
    }

    /// The file being watched.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// When the file changed since the previous call, its new contents (or
    /// why they cannot be used, e.g. a file saved half-way). `None` when
    /// unchanged, checked less than a second ago, or removed: a missing
    /// file keeps the configuration in use rather than resetting it.
    pub fn poll(&mut self) -> Option<Result<Config, ConfigError>> {
        if self.last_check.elapsed() < self.interval {
            return None;
        }
        self.last_check = Instant::now();
        let stamp = file_stamp(&self.path);
        if stamp == self.stamp {
            return None;
        }
        self.stamp = stamp;
        stamp?;
        Some(Config::load(&self.path))
    }
}

/// Modification time and size of `path`, if it exists.
fn file_stamp(path: &Path) -> Option<(Option<SystemTime>, u64)> {
    let metadata = fs::metadata(path).ok()?;
    Some((metadata.modified().ok(), metadata.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_checks_can_be_tuned_or_turned_off() {
        let config: Config =
            toml::from_str("[updates]\ninterval = \"6h\"\nnotify = false\n").unwrap();
        assert!(config.updates.check);
        assert_eq!(
            config.updates.check_interval(),
            Duration::from_secs(6 * 3600)
        );
        assert!(!config.updates.notify);
        let config: Config =
            toml::from_str("[updates]\ncheck = false\ninterval = \"10s\"\n").unwrap();
        assert!(!config.updates.check);
        assert_eq!(
            config.updates.check_interval(),
            Duration::from_secs(60),
            "at least a minute"
        );
        let error = toml::from_str::<Config>("[updates]\ninterval = \"6 hours\"\n").unwrap_err();
        assert!(error.to_string().contains("invalid duration"), "{error}");
    }

    #[test]
    fn durations_read_and_write_as_text() {
        let minutes = |m: u64| Some(Duration::from_secs(m * 60));
        assert_eq!(duration_text::parse("30m"), minutes(30));
        assert_eq!(duration_text::parse("1h30m"), minutes(90));
        assert_eq!(duration_text::parse("1h 30m"), minutes(90));
        assert_eq!(duration_text::parse("1d"), minutes(24 * 60));
        assert_eq!(duration_text::parse("2H"), minutes(120));
        for bad in ["", "6", "h", "1.5h", "1w", "-1h", "6 hours"] {
            assert_eq!(duration_text::parse(bad), None, "{bad:?}");
        }
        assert_eq!(duration_text::format(Duration::from_secs(5400)), "1h30m");
        assert_eq!(duration_text::format(Duration::from_secs(86_400)), "1d");
        // The defaults survive saving and loading.
        let text = toml::to_string(&Config::default()).unwrap();
        assert!(text.contains("interval = \"1h\""), "{text}");
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), Config::default());
    }

    #[test]
    fn watcher_reports_edits_once() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, "[alerts]\ncpu_warning_celsius = 90\n").unwrap();
        let mut watcher = ConfigWatcher::new(path.clone());
        watcher.interval = Duration::ZERO;
        assert!(
            watcher.poll().is_none(),
            "the initial file counts as loaded"
        );

        // A different size, so the change shows even where file times are coarse.
        fs::write(&path, "[alerts]\ncpu_warning_celsius = 100\n").unwrap();
        let config = watcher.poll().unwrap().unwrap();
        assert_eq!(config.alerts.cpu_warning_celsius, 100.0);
        assert!(watcher.poll().is_none(), "reported once");

        fs::write(&path, "[alerts\n").unwrap();
        assert!(
            watcher.poll().unwrap().is_err(),
            "a broken file is an error"
        );
        fs::remove_file(&path).unwrap();
        assert!(
            watcher.poll().is_none(),
            "removing the file changes nothing"
        );
    }

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

[lhm]
url = ""

[notifications]
level = "warning"

[alerts]
drive_warning_celsius = 65
cpu_warning_celsius = 0
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
        assert_eq!(config.lhm.url, "");
        assert_eq!(config.notifications.level, NotifyLevel::Warning);
        assert_eq!(Config::default().notifications.level, NotifyLevel::Critical);
        assert_eq!(Config::default().lhm.url, DEFAULT_LHM_URL);
        assert_eq!(config.alerts.drive_warning_celsius, 65.0);
        assert_eq!(config.alerts.cpu_warning_celsius, 0.0);
        // Unset thresholds keep their defaults.
        assert_eq!(config.alerts.gpu_warning_celsius, 85.0);
    }

    #[test]
    fn parses_hosts_and_overview() {
        let config: Config = toml::from_str(
            r#"
[overview]
sections = ["storage", "cpu"]
include_local = false

[[hosts]]
name = "zefat"

[[hosts]]
name = "nas"
ssh = "admin@nas.local"
sections = ["storage"]

[[widgets]]
name = "Clock"
builtin = "clock"

[[widgets]]
name = "Rates"
command = "rates.sh"
interval_secs = 60
"#,
        )
        .unwrap();
        assert_eq!(
            config.overview.sections,
            vec![OverviewSection::Storage, OverviewSection::Cpu]
        );
        assert!(!config.overview.include_local);
        assert_eq!(config.overview.processes, 10, "default kept");
        assert_eq!(config.hosts.len(), 2);
        assert_eq!(config.hosts[0].destination(), "zefat");
        assert_eq!(config.widgets[0].builtin, Some(BuiltinWidget::Clock));
        assert_eq!(config.widgets[1].command.as_deref(), Some("rates.sh"));
        assert_eq!(config.widgets[1].interval_secs, 60);
        assert_eq!(config.widgets[1].timeout_secs, 10, "default kept");
        assert_eq!(config.hosts[1].destination(), "admin@nas.local");
        assert_eq!(
            config.hosts[1].sections,
            Some(vec![OverviewSection::Storage])
        );
        // Defaults: every section in priority order, local included.
        let defaults = Config::default();
        assert_eq!(defaults.overview.sections, OverviewSection::ALL.to_vec());
        assert!(defaults.overview.include_local);
    }

    #[test]
    fn save_hosts_replaces_only_the_hosts() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(
            &path,
            "# my settings\n[sensors]\nlabels = { \"a/fan1\" = \"CPU fan\" } # keep me\n\n[[hosts]]\nname = \"old\"\n",
        )
        .unwrap();
        let hosts = vec![
            HostConfig::from_destination("zefat"),
            HostConfig {
                name: "nas".to_owned(),
                ssh: Some("admin@nas.local".to_owned()),
                command: Some("/opt/rigtop --agent".to_owned()),
                sections: Some(vec![OverviewSection::Storage, OverviewSection::Cpu]),
            },
        ];
        Config::save_hosts(&path, &hosts).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("# my settings") && text.contains("# keep me"),
            "{text}"
        );
        assert!(!text.contains("\"old\""), "{text}");
        let config = Config::load(&path).unwrap();
        assert_eq!(config.hosts, hosts);
        assert_eq!(config.label_for("a/fan1"), Some("CPU fan"));

        // Removing every host drops the [[hosts]] tables.
        Config::save_hosts(&path, &[]).unwrap();
        assert!(Config::load(&path).unwrap().hosts.is_empty());
        assert!(fs::read_to_string(&path).unwrap().contains("# keep me"));
    }

    #[test]
    fn save_sensors_replaces_only_names_and_hidden_sensors() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(
            &path,
            "# my settings\n[sensors]\nlabels = { \"a/fan1\" = \"CPU fan\" } # inline\n\n[[hosts]]\nname = \"zefat\"\n",
        )
        .unwrap();
        let mut sensors = SensorConfig::default();
        sensors
            .labels
            .insert("a/fan1".to_owned(), "CPU fan".to_owned());
        sensors
            .labels
            .insert("b:c.1/temp2".to_owned(), "VRM".to_owned());
        sensors.hidden.push("b:c.1/temp4".to_owned());
        Config::save_sensors(&path, &sensors).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my settings"), "{text}");
        // Inline names stay inline.
        assert!(text.contains("labels = {"), "{text}");
        let config = Config::load(&path).unwrap();
        assert_eq!(config.sensors, sensors);
        assert_eq!(config.hosts[0].name, "zefat");

        // Nothing left: the [sensors] table goes away.
        Config::save_sensors(&path, &SensorConfig::default()).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("[sensors]"), "{text}");
        assert_eq!(Config::load(&path).unwrap().hosts.len(), 1);

        // A new file gets a [sensors.labels] table.
        let fresh = tmp.path().join("new.toml");
        Config::save_sensors(&fresh, &sensors).unwrap();
        let text = fs::read_to_string(&fresh).unwrap();
        assert!(text.contains("[sensors.labels]"), "{text}");
        assert_eq!(Config::load(&fresh).unwrap().sensors, sensors);
    }

    #[test]
    fn save_hosts_creates_a_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("new/rigtop/config.toml");
        Config::save_hosts(&path, &[HostConfig::from_destination("zefat")]).unwrap();
        assert_eq!(Config::load(&path).unwrap().hosts[0].name, "zefat");
    }

    #[test]
    fn save_hosts_refuses_to_touch_a_broken_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, "[sensors\n").unwrap();
        let err = Config::save_hosts(&path, &[]).unwrap_err();
        assert!(matches!(err, ConfigError::Edit { .. }), "{err}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "[sensors\n");
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
