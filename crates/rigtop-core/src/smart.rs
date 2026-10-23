//! SMART health from smartctl JSON files.
//!
//! Reading SMART data needs root, and rigtop never runs as root nor loosens
//! device permissions. Instead a root systemd timer (see `contrib/smart/`)
//! runs `smartctl -j -a` for every drive and writes one JSON file per device
//! into a directory (default `/var/lib/rigtop/smart`). This module only parses
//! those files.
//!
//! Files are re-parsed only when their modification time changes, so polling
//! every second costs a `stat` per drive.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use crate::snapshot::SmartDevice;

/// NVMe "data units" are 1000 blocks of 512 bytes.
const NVME_DATA_UNIT_BYTES: u64 = 512_000;

/// smartctl exit status bits meaning the data could not be read at all
/// (bit 0: command line error, bit 1: device open failed).
const EXIT_FATAL_MASK: u64 = 0b11;

/// Reads and caches SMART result files from a directory.
#[derive(Debug)]
pub struct SmartReader {
    dir: PathBuf,
    /// Parsed result and the modification time it was parsed at, per file.
    cache: HashMap<PathBuf, (SystemTime, SmartDevice)>,
}

impl SmartReader {
    /// Creates a reader for `dir`. The directory may not exist (SMART collection
    /// not installed); then no devices are reported.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            cache: HashMap::new(),
        }
    }

    /// Returns the SMART state of every `*.json` file in the directory, sorted by device.
    pub fn read(&mut self) -> Vec<SmartDevice> {
        let mut paths: Vec<PathBuf> = match fs::read_dir(&self.dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|ext| ext == "json"))
                .collect(),
            Err(_) => Vec::new(),
        };
        paths.sort();

        // Forget files that disappeared (drive removed).
        self.cache.retain(|path, _| paths.contains(path));

        paths
            .into_iter()
            .filter_map(|path| {
                let mtime = fs::metadata(&path).and_then(|m| m.modified()).ok()?;
                if let Some((cached_mtime, device)) = self.cache.get(&path)
                    && *cached_mtime == mtime
                {
                    return Some(device.clone());
                }
                let device = parse_file(&path);
                self.cache.insert(path, (mtime, device.clone()));
                Some(device)
            })
            .collect()
    }
}

/// Parses one result file. Unreadable or malformed files become an entry with
/// `error` set, so that a broken collector is visible rather than silent.
fn parse_file(path: &Path) -> SmartDevice {
    // The collector names files after the device ("nvme0.json"), with a
    // suffix for drives behind RAID controllers ("0_megaraid_3.json") whose
    // device node is shared, so the file name is the better identifier.
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let parsed = fs::read_to_string(path)
        .map_err(|e| e.to_string())
        .and_then(|text| serde_json::from_str::<Value>(&text).map_err(|e| e.to_string()));
    match parsed {
        Ok(json) => parse_json(&json, &name),
        Err(err) => SmartDevice {
            device: name,
            error: Some(format!("cannot read SMART result: {err}")),
            ..Default::default()
        },
    }
}

/// Extracts the fields rigtop shows from `smartctl -j -a` output.
///
/// `device` names the drive; when empty, the last component of the JSON's
/// `device.name` (e.g. `/dev/nvme0` -> `nvme0`) is used.
pub fn parse_json(json: &Value, device: &str) -> SmartDevice {
    let u64_at = |pointer: &str| json.pointer(pointer).and_then(Value::as_u64);
    let u32_at = |pointer: &str| u64_at(pointer).and_then(|v| u32::try_from(v).ok());
    let str_at = |pointer: &str| json.pointer(pointer).and_then(Value::as_str);

    let device = if device.is_empty() {
        str_at("/device/name")
            .and_then(|name| name.rsplit('/').next())
            .unwrap_or_default()
            .to_owned()
    } else {
        device.to_owned()
    };

    let exit_status = u64_at("/smartctl/exit_status").unwrap_or(0);
    let error = (exit_status & EXIT_FATAL_MASK != 0).then(|| {
        // Prefer smartctl's own message (e.g. "... Permission denied").
        json.pointer("/smartctl/messages")
            .and_then(Value::as_array)
            .and_then(|messages| {
                messages
                    .iter()
                    .find(|m| m.get("severity").and_then(Value::as_str) == Some("error"))
                    .and_then(|m| m.get("string").and_then(Value::as_str))
            })
            .map(str::to_owned)
            .unwrap_or_else(|| format!("smartctl failed (exit status {exit_status})"))
    });

    // ATA attributes are a table of {id, raw: {value}} entries.
    let ata_raw = |id: u64| {
        json.pointer("/ata_smart_attributes/table")
            .and_then(Value::as_array)?
            .iter()
            .find(|attr| attr.get("id").and_then(Value::as_u64) == Some(id))?
            .pointer("/raw/value")
            .and_then(Value::as_u64)
    };

    let nvme = "/nvme_smart_health_information_log";
    SmartDevice {
        device,
        protocol: str_at("/device/protocol").unwrap_or_default().to_owned(),
        model: str_at("/model_name").unwrap_or_default().trim().to_owned(),
        passed: json
            .pointer("/smart_status/passed")
            .and_then(Value::as_bool),
        temperature_c: u64_at("/temperature/current").map(|t| t as f32),
        power_on_hours: u64_at("/power_on_time/hours"),
        percentage_used: u32_at(&format!("{nvme}/percentage_used")),
        available_spare_percent: u32_at(&format!("{nvme}/available_spare")),
        available_spare_threshold_percent: u32_at(&format!("{nvme}/available_spare_threshold")),
        critical_warning: u32_at(&format!("{nvme}/critical_warning")),
        media_errors: u64_at(&format!("{nvme}/media_errors")),
        reallocated_sectors: ata_raw(5),
        pending_sectors: ata_raw(197),
        offline_uncorrectable: ata_raw(198),
        data_written_bytes: u64_at(&format!("{nvme}/data_units_written"))
            .map(|units| units.saturating_mul(NVME_DATA_UNIT_BYTES)),
        collected_at_ms: u64_at("/local_time/time_t").map(|t| t.saturating_mul(1000)),
        error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed `smartctl -j -a /dev/nvme0` output (smartctl 7.5).
    const NVME_JSON: &str = r#"{
      "json_format_version": [1, 0],
      "smartctl": {"version": [7, 5], "exit_status": 0},
      "local_time": {"time_t": 1792775225, "asctime": "Sat Oct 24 02:07:05 2026 JST"},
      "device": {"name": "/dev/nvme0", "info_name": "/dev/nvme0", "type": "nvme", "protocol": "NVMe"},
      "model_name": "Samsung SSD 990 PRO 2TB",
      "serial_number": "S000000000000",
      "smart_status": {"passed": true, "nvme": {"value": 0}},
      "nvme_smart_health_information_log": {
        "critical_warning": 0, "temperature": 45, "available_spare": 100,
        "available_spare_threshold": 10, "percentage_used": 2,
        "data_units_read": 12345678, "data_units_written": 2000000,
        "power_on_hours": 3210, "unsafe_shutdowns": 12, "media_errors": 0,
        "num_err_log_entries": 0
      },
      "temperature": {"current": 45},
      "power_on_time": {"hours": 3210}
    }"#;

    /// Trimmed `smartctl -j -a /dev/sda` output for a SATA disk with problems.
    const ATA_JSON: &str = r#"{
      "smartctl": {"exit_status": 64},
      "local_time": {"time_t": 1792775225},
      "device": {"name": "/dev/sda", "type": "sat", "protocol": "ATA"},
      "model_name": "WDC WD40EFRX-68N32N0",
      "smart_status": {"passed": true},
      "ata_smart_attributes": {"revision": 16, "table": [
        {"id": 5, "name": "Reallocated_Sector_Ct", "raw": {"value": 8, "string": "8"}},
        {"id": 9, "name": "Power_On_Hours", "raw": {"value": 40000, "string": "40000"}},
        {"id": 197, "name": "Current_Pending_Sector", "raw": {"value": 0, "string": "0"}},
        {"id": 198, "name": "Offline_Uncorrectable", "raw": {"value": 0, "string": "0"}}
      ]},
      "temperature": {"current": 38},
      "power_on_time": {"hours": 40000}
    }"#;

    /// smartctl output when run without root.
    const DENIED_JSON: &str = r#"{
      "smartctl": {"messages": [{"string": "Smartctl open device: /dev/nvme0 failed: Permission denied", "severity": "error"}], "exit_status": 2},
      "local_time": {"time_t": 1792775225},
      "device": {"name": "/dev/nvme0", "type": "nvme", "protocol": "NVMe"}
    }"#;

    fn parse(text: &str) -> SmartDevice {
        parse_json(&serde_json::from_str(text).unwrap(), "")
    }

    #[test]
    fn parses_nvme_health() {
        let d = parse(NVME_JSON);
        assert_eq!(d.device, "nvme0");
        assert_eq!(d.protocol, "NVMe");
        assert_eq!(d.model, "Samsung SSD 990 PRO 2TB");
        assert_eq!(d.passed, Some(true));
        assert_eq!(d.temperature_c, Some(45.0));
        assert_eq!(d.percentage_used, Some(2));
        assert_eq!(d.available_spare_percent, Some(100));
        assert_eq!(d.media_errors, Some(0));
        assert_eq!(d.data_written_bytes, Some(2_000_000 * 512_000));
        assert_eq!(d.collected_at_ms, Some(1_792_775_225_000));
        assert_eq!(d.reallocated_sectors, None);
        assert!(d.error.is_none());
        assert!(!d.needs_attention());
    }

    #[test]
    fn parses_ata_attributes() {
        let d = parse(ATA_JSON);
        assert_eq!(d.device, "sda");
        assert_eq!(d.reallocated_sectors, Some(8));
        assert_eq!(d.pending_sectors, Some(0));
        assert_eq!(d.power_on_hours, Some(40000));
        // Exit status 64 (error log entries) is a warning, not a read failure.
        assert!(d.error.is_none());
        assert!(d.needs_attention());
    }

    #[test]
    fn open_failure_is_reported_as_error() {
        let d = parse(DENIED_JSON);
        assert_eq!(d.passed, None);
        assert!(d.error.as_deref().unwrap().contains("Permission denied"));
        assert!(d.needs_attention());
    }

    #[test]
    fn reader_caches_by_mtime_and_reports_broken_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        fs::write(dir.join("nvme0.json"), NVME_JSON).unwrap();
        fs::write(dir.join("sdb.json"), "{ truncated").unwrap();
        fs::write(dir.join("0_megaraid_3.json"), DENIED_JSON).unwrap();
        fs::write(dir.join("README"), "not a result").unwrap();

        let mut reader = SmartReader::new(dir);
        let devices = reader.read();
        let names: Vec<&str> = devices.iter().map(|d| d.device.as_str()).collect();
        // Devices are named after their files, not the shared device node.
        assert_eq!(names, vec!["0_megaraid_3", "nvme0", "sdb"]);
        // A malformed file still shows up, carrying the error.
        assert!(devices[2].error.is_some());

        // Removing a file drops it from the result and the cache.
        fs::remove_file(dir.join("sdb.json")).unwrap();
        let devices = reader.read();
        assert_eq!(devices.len(), 2);
        assert_eq!(reader.cache.len(), 2);
    }

    #[test]
    fn missing_directory_yields_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let mut reader = SmartReader::new(tmp.path().join("absent"));
        assert!(reader.read().is_empty());
    }
}
