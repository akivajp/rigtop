//! Temperatures, fans and CPU power on Windows, from LibreHardwareMonitor.
//!
//! Windows has no unprivileged interface for these readings, so rigtop reads
//! them from LibreHardwareMonitor (LHM), which runs elevated with its own
//! driver. LHM dropped its WMI provider in v0.9.5; what remains is the
//! `data.json` of its "Remote Web Server", a tree of
//! computer → hardware → sensor type → sensor nodes:
//!
//! ```json
//! {"Text": "AMD Ryzen 9 8945HX", "HardwareId": "/amdcpu/0", "Children": [
//!   {"Text": "Temperatures", "Children": [
//!     {"Text": "Core (Tctl/Tdie)", "SensorId": "/amdcpu/0/temperature/2",
//!      "Type": "Temperature", "Value": "52.4 °C", "RawValue": 52.375}]}]}
//! ```

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::snapshot::{Fan, SensorChip, SmartDevice, Temperature};

/// How long to wait for LHM before giving up on this sample.
const TIMEOUT: Duration = Duration::from_millis(800);
/// How long to wait before trying again after LHM was unreachable.
const RETRY_AFTER: Duration = Duration::from_secs(10);
/// Upper bound for the response, against a misconfigured URL.
const MAX_RESPONSE: u64 = 16 * 1024 * 1024;

/// Prefix of the chip keys, to tell them apart from hwmon chips.
const KEY_PREFIX: &str = "lhm:";

/// What rigtop takes from LHM.
#[derive(Debug, Default, Clone)]
pub struct LhmReadings {
    /// One chip per hardware item with temperatures or fans.
    pub chips: Vec<SensorChip>,
    /// The CPU's temperatures, with short labels (`Tctl`, `Tccd1`, `Package`).
    pub cpu_temperatures: Vec<Temperature>,
    /// CPU package power.
    pub cpu_package_w: Option<f32>,
    /// Drive health, from LHM's storage sensors (SMART read by its driver).
    pub smart: Vec<SmartDevice>,
}

/// Polls LHM's web server, backing off while it is unreachable.
pub struct LhmReader {
    url: String,
    retry_at: Option<Instant>,
    /// Why the last attempt failed, kept while waiting to retry.
    problem: Option<String>,
}

impl LhmReader {
    /// Reads from `url` (the `data.json` address); an empty URL disables it.
    pub fn new(url: &str) -> Self {
        Self {
            url: url.trim().to_owned(),
            retry_at: None,
            problem: None,
        }
    }

    /// Why LHM could not be read, until it can.
    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// Current readings, or `None` if disabled or LHM is not reachable.
    pub fn read(&mut self, now: Instant) -> Option<LhmReadings> {
        if self.url.is_empty() || self.retry_at.is_some_and(|at| now < at) {
            return None;
        }
        match http_get(&self.url, TIMEOUT).and_then(|body| parse(&body)) {
            Ok(readings) => {
                self.retry_at = None;
                self.problem = None;
                Some(readings)
            }
            Err(err) => {
                // Usually LHM is not running or its web server is off; do
                // not slow every sample down with a failing request.
                self.retry_at = Some(now + RETRY_AFTER);
                self.problem = Some(err);
                None
            }
        }
    }
}

/// A node of `data.json`. Only the fields rigtop uses; older LHM versions
/// lack `SensorId`, `Type` and `RawValue`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Node {
    #[serde(rename = "Text")]
    text: String,
    #[serde(rename = "HardwareId")]
    hardware_id: Option<String>,
    #[serde(rename = "SensorId")]
    sensor_id: Option<String>,
    #[serde(rename = "Type")]
    kind: Option<String>,
    /// Formatted value such as `"52.4 °C"` (locale-dependent decimal mark).
    #[serde(rename = "Value")]
    value: String,
    /// Unformatted value; null when the sensor has no reading.
    #[serde(rename = "RawValue")]
    raw_value: serde_json::Value,
    #[serde(rename = "Children")]
    children: Vec<Node>,
}

/// Sensor types rigtop uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Temperature,
    Fan,
    Power,
}

impl Kind {
    /// From the `Type` field, the type in the sensor id
    /// (`/amdcpu/0/temperature/2`) or the group node (`Temperatures`).
    fn of(sensor: &Node, group: Option<&Node>) -> Option<Self> {
        let from_name = |name: &str| match name.to_ascii_lowercase().trim_end_matches('s') {
            "temperature" => Some(Kind::Temperature),
            "fan" => Some(Kind::Fan),
            "power" => Some(Kind::Power),
            _ => None,
        };
        if let Some(kind) = &sensor.kind {
            return from_name(kind);
        }
        if let Some(id) = &sensor.sensor_id {
            let mut segments = id.rsplit('/');
            segments.next();
            return segments.next().and_then(from_name);
        }
        group.and_then(|g| from_name(&g.text))
    }
}

/// Parses `data.json`.
fn parse(body: &[u8]) -> Result<LhmReadings, String> {
    let root: Node = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    let mut readings = LhmReadings::default();
    let mut ancestors = Vec::new();
    walk(&root, &mut ancestors, &mut readings);
    readings
        .chips
        .retain(|c| !c.temperatures.is_empty() || !c.fans.is_empty());
    Ok(readings)
}

fn walk<'a>(node: &'a Node, ancestors: &mut Vec<&'a Node>, out: &mut LhmReadings) {
    if node.children.is_empty() {
        if !ancestors.is_empty() {
            add_sensor(node, ancestors, out);
        }
        return;
    }
    ancestors.push(node);
    for child in &node.children {
        walk(child, ancestors, out);
    }
    ancestors.pop();
}

fn add_sensor(sensor: &Node, ancestors: &[&Node], out: &mut LhmReadings) {
    let group = ancestors.last().copied();
    let Some(value) = sensor_value(sensor) else {
        return;
    };
    // The nearest hardware node; old versions have no ids, so the parent of
    // the group node stands in.
    let hardware = ancestors
        .iter()
        .rev()
        .find(|n| n.hardware_id.is_some())
        .or_else(|| ancestors.len().checked_sub(2).map(|i| &ancestors[i]));
    let Some(hardware) = hardware else {
        return;
    };
    let hardware_id = hardware.hardware_id.as_deref().unwrap_or(&hardware.text);
    if let Some((transport, number)) = storage_disk(hardware_id) {
        add_storage_sensor(out, &hardware.text, transport, number, &sensor.text, value);
    }
    let Some(kind) = Kind::of(sensor, group) else {
        return;
    };
    // NVML reports NVIDIA GPUs in more detail.
    if hardware_id.starts_with("/gpu-nvidia") {
        return;
    }
    let is_cpu = ["/amdcpu", "/intelcpu", "/cpu"]
        .iter()
        .any(|p| hardware_id.starts_with(p));
    let chip_key = format!("{KEY_PREFIX}{hardware_id}");
    let key = channel_key(&chip_key, sensor, kind);

    if kind == Kind::Power {
        if is_cpu && matches!(sensor.text.as_str(), "Package" | "CPU Package") {
            out.cpu_package_w = Some(value as f32);
        }
        return;
    }
    let index = match out.chips.iter().position(|c| c.key == chip_key) {
        Some(index) => index,
        None => {
            out.chips.push(SensorChip {
                key: chip_key,
                name: hardware.text.clone(),
                temperatures: Vec::new(),
                fans: Vec::new(),
            });
            out.chips.len() - 1
        }
    };
    let chip = &mut out.chips[index];
    match kind {
        Kind::Temperature => {
            let temperature = Temperature {
                key,
                label: sensor.text.clone(),
                celsius: value as f32,
                max_celsius: None,
                critical_celsius: None,
            };
            if is_cpu && let Some(label) = cpu_label(&sensor.text, hardware_id) {
                out.cpu_temperatures.push(Temperature {
                    label,
                    ..temperature.clone()
                });
            }
            chip.temperatures.push(temperature);
        }
        Kind::Fan => chip.fans.push(Fan {
            key,
            label: sensor.text.clone(),
            rpm: value.max(0.0).round() as u32,
        }),
        Kind::Power => {}
    }
}

/// A drive's hardware id, `/nvme/0` or `/ata/1`: its transport and Windows
/// disk number. `None` for other hardware (and virtual disks).
fn storage_disk(hardware_id: &str) -> Option<(&str, &str)> {
    let mut parts = hardware_id.trim_start_matches('/').split('/');
    let (transport, number) = (parts.next()?, parts.next()?);
    let known = matches!(
        transport,
        "nvme" | "ata" | "ahci" | "scsi" | "sas" | "raid" | "usb" | "sd" | "mmc" | "disk"
    );
    (known && parts.next().is_none()).then_some((transport, number))
}

/// Folds one storage sensor into the drive's [`SmartDevice`], named
/// `disk<N>` like Disk Management's "Disk N".
fn add_storage_sensor(
    out: &mut LhmReadings,
    model: &str,
    transport: &str,
    number: &str,
    sensor: &str,
    value: f64,
) {
    let device = format!("disk{number}");
    let index = match out.smart.iter().position(|d| d.device == device) {
        Some(index) => index,
        None => {
            out.smart.push(SmartDevice {
                protocol: match transport {
                    "nvme" => "NVMe".to_owned(),
                    "ata" | "ahci" => "ATA".to_owned(),
                    other => other.to_uppercase(),
                },
                device,
                model: model.to_owned(),
                ..Default::default()
            });
            out.smart.len() - 1
        }
    };
    let drive = &mut out.smart[index];
    let whole = value.max(0.0).round();
    match sensor {
        "Temperature" => drive.temperature_c = Some(value as f32),
        // Some NVMe drives only have numbered sensors ("Temperature #1");
        // take the first unless the main reading comes.
        name if name.starts_with("Temperature #") => {
            drive.temperature_c.get_or_insert(value as f32);
        }
        "Power On Hours" => drive.power_on_hours = Some(whole as u64),
        // "Life" is the remaining share; NVMe's own "Percentage Used" (listed
        // after it) replaces this estimate.
        "Life" | "Remaining Life" => {
            drive.percentage_used = Some((100.0 - whole).max(0.0) as u32);
        }
        "Percentage Used" => drive.percentage_used = Some(whole as u32),
        "Available Spare" => drive.available_spare_percent = Some(whole as u32),
        "Available Spare Threshold" => {
            drive.available_spare_threshold_percent = Some(whole as u32);
        }
        _ => {}
    }
}

/// `RawValue`, else the number at the start of the formatted `Value`.
fn sensor_value(sensor: &Node) -> Option<f64> {
    if let Some(value) = sensor.raw_value.as_f64() {
        return value.is_finite().then_some(value);
    }
    let number: String = sensor
        .value
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '-'))
        .map(|c| if c == ',' { '.' } else { c })
        .collect();
    number.parse().ok()
}

/// `"<chip key>/temp2"` from the sensor id `/amdcpu/0/temperature/2`, like
/// the hwmon keys; the sensor name when there is no id.
fn channel_key(chip_key: &str, sensor: &Node, kind: Kind) -> String {
    let prefix = match kind {
        Kind::Temperature => "temp",
        Kind::Fan => "fan",
        Kind::Power => "power",
    };
    match sensor
        .sensor_id
        .as_deref()
        .and_then(|id| id.rsplit('/').next())
    {
        Some(index) => format!("{chip_key}/{prefix}{index}"),
        None => format!("{chip_key}/{}", sensor.text),
    }
}

/// Short label for a CPU temperature, or `None` for readings left out of
/// the CPU line (derived maxima and averages, Intel's distance to TjMax,
/// and AMD's per-core readings next to the per-CCD ones).
fn cpu_label(name: &str, hardware_id: &str) -> Option<String> {
    if name.contains("Max") || name.contains("Average") || name.contains("Distance") {
        return None;
    }
    let label = match name {
        "Core (Tctl/Tdie)" | "Core (Tctl)" => "Tctl".to_owned(),
        "Core (Tdie)" => "Tdie".to_owned(),
        "CPU Package" => "Package".to_owned(),
        _ => {
            // "CCD1 (Tdie)" -> "Tccd1"
            if let Some(ccd) = name
                .strip_prefix("CCD")
                .and_then(|r| r.strip_suffix(" (Tdie)"))
            {
                format!("Tccd{ccd}")
            } else if let Some(core) = name.strip_prefix("Core #") {
                if hardware_id.starts_with("/amdcpu") {
                    return None;
                }
                format!("Core {core}")
            } else {
                name.to_owned()
            }
        }
    };
    Some(label)
}

/// A minimal HTTP/1.0 GET for `http://host[:port]/path`, enough for LHM's
/// local web server (no TLS, redirects or authentication).
fn http_get(url: &str, timeout: Duration) -> Result<Vec<u8>, String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("only http:// URLs are supported: {url}"))?;
    let (authority, path) = match rest.find('/') {
        Some(slash) => (&rest[..slash], &rest[slash..]),
        None => (rest, "/"),
    };
    let address = if authority.contains(':') {
        authority.to_owned()
    } else {
        format!("{authority}:80")
    };
    let socket = address
        .to_socket_addrs()
        .map_err(|e| format!("{authority}: {e}"))?
        .next()
        .ok_or_else(|| format!("{authority}: no address"))?;
    let mut stream =
        TcpStream::connect_timeout(&socket, timeout).map_err(|e| format!("{authority}: {e}"))?;
    stream
        .set_read_timeout(Some(timeout))
        .and_then(|_| stream.set_write_timeout(Some(timeout)))
        .map_err(|e| e.to_string())?;
    // HTTP/1.0 keeps the response simple: no chunked encoding, and the
    // server closes the connection at the end.
    let request =
        format!("GET {path} HTTP/1.0\r\nHost: {authority}\r\nAccept: application/json\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|e| e.to_string())?;

    let mut reader = BufReader::new(stream.take(MAX_RESPONSE));
    let mut status = String::new();
    reader.read_line(&mut status).map_err(|e| e.to_string())?;
    let code = status.split_whitespace().nth(1).unwrap_or("");
    if code != "200" {
        return Err(format!("HTTP status {:?}", status.trim()));
    }
    // Skip the headers.
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).map_err(|e| e.to_string())?;
        if read == 0 || line == "\r\n" || line == "\n" {
            break;
        }
    }
    let mut body = Vec::new();
    reader.read_to_end(&mut body).map_err(|e| e.to_string())?;
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    /// Shaped like LHM 0.9.6 output, trimmed.
    const SAMPLE: &str = r#"{"id":0,"Text":"Sensor","Min":"Min","Value":"Value","Max":"Max","ImageURL":"","Children":[
      {"id":1,"Text":"CARMEL","Min":"","Value":"","Max":"","ImageURL":"images_icon/computer.png","Children":[
        {"id":2,"Text":"AMD Ryzen 9 8945HX","HardwareId":"/amdcpu/0","Value":"","Children":[
          {"id":3,"Text":"Temperatures","Value":"","Children":[
            {"id":4,"Text":"Core (Tctl/Tdie)","SensorId":"/amdcpu/0/temperature/2","Type":"Temperature","Value":"52.4 °C","RawValue":52.375,"Children":[]},
            {"id":5,"Text":"CCD1 (Tdie)","SensorId":"/amdcpu/0/temperature/3","Type":"Temperature","Value":"48.1 °C","RawValue":48.125,"Children":[]},
            {"id":6,"Text":"CCDs Max (Tdie)","SensorId":"/amdcpu/0/temperature/5","Type":"Temperature","Value":"48.1 °C","RawValue":48.125,"Children":[]},
            {"id":7,"Text":"Core #1","SensorId":"/amdcpu/0/temperature/7","Type":"Temperature","Value":"45.0 °C","RawValue":45.0,"Children":[]}]},
          {"id":8,"Text":"Powers","Value":"","Children":[
            {"id":9,"Text":"Package","SensorId":"/amdcpu/0/power/0","Type":"Power","Value":"35.2 W","RawValue":35.2,"Children":[]},
            {"id":10,"Text":"Core #1 (SMU)","SensorId":"/amdcpu/0/power/1","Type":"Power","Value":"1.2 W","RawValue":1.2,"Children":[]}]},
          {"id":11,"Text":"Load","Value":"","Children":[
            {"id":12,"Text":"CPU Total","SensorId":"/amdcpu/0/load/0","Type":"Load","Value":"3.0 %","RawValue":3.0,"Children":[]}]}]},
        {"id":13,"Text":"ASUS ROG STRIX","HardwareId":"/motherboard","Value":"","Children":[
          {"id":14,"Text":"Nuvoton NCT6799D","HardwareId":"/lpc/nct6799d/0","Value":"","Children":[
            {"id":15,"Text":"Fans","Value":"","Children":[
              {"id":16,"Text":"Fan #1","SensorId":"/lpc/nct6799d/0/fan/0","Type":"Fan","Value":"918 RPM","RawValue":918.4,"Children":[]},
              {"id":17,"Text":"Fan #2","SensorId":"/lpc/nct6799d/0/fan/1","Type":"Fan","Value":"-","RawValue":null,"Children":[]}]},
            {"id":18,"Text":"Temperatures","Value":"","Children":[
              {"id":19,"Text":"System","SensorId":"/lpc/nct6799d/0/temperature/1","Type":"Temperature","Value":"34.0 °C","RawValue":34.0,"Children":[]}]}]}]},
        {"id":20,"Text":"NVIDIA GeForce RTX 4090","HardwareId":"/gpu-nvidia/0","Value":"","Children":[
          {"id":21,"Text":"Temperatures","Value":"","Children":[
            {"id":22,"Text":"GPU Core","SensorId":"/gpu-nvidia/0/temperature/0","Type":"Temperature","Value":"59.0 °C","RawValue":59.0,"Children":[]}]}]},
        {"id":23,"Text":"Generic Memory","HardwareId":"/ram","Value":"","Children":[
          {"id":24,"Text":"Load","Value":"","Children":[
            {"id":25,"Text":"Memory","SensorId":"/ram/load/0","Type":"Load","Value":"46.0 %","RawValue":46.0,"Children":[]}]}]}]}]}"#;

    #[test]
    fn parses_cpu_motherboard_and_skips_nvidia() {
        let readings = parse(SAMPLE.as_bytes()).unwrap();
        let labels: Vec<&str> = readings
            .cpu_temperatures
            .iter()
            .map(|t| t.label.as_str())
            .collect();
        assert_eq!(labels, ["Tctl", "Tccd1"], "no maxima or AMD per-core");
        assert_eq!(readings.cpu_temperatures[0].key, "lhm:/amdcpu/0/temp2");
        assert_eq!(readings.cpu_package_w, Some(35.2));

        let keys: Vec<&str> = readings.chips.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(
            keys,
            ["lhm:/amdcpu/0", "lhm:/lpc/nct6799d/0"],
            "NVIDIA comes from NVML; memory has no temperatures"
        );
        let board = &readings.chips[1];
        assert_eq!(board.name, "Nuvoton NCT6799D");
        assert_eq!(board.fans.len(), 1, "a fan without a reading is left out");
        assert_eq!(board.fans[0].key, "lhm:/lpc/nct6799d/0/fan0");
        assert_eq!(board.fans[0].rpm, 918);
        assert_eq!(board.temperatures[0].label, "System");
        // The CPU chip keeps LHM's full names.
        assert_eq!(readings.chips[0].temperatures[0].label, "Core (Tctl/Tdie)");
    }

    #[test]
    fn storage_sensors_become_smart_devices() {
        let sensor = |id: &str, kind: &str, name: &str, value: f64| {
            format!(
                r#"{{"Text":"{name}","SensorId":"{id}","Type":"{kind}","Value":"","RawValue":{value},"Children":[]}}"#
            )
        };
        let body = format!(
            r#"{{"Text":"Sensor","Children":[{{"Text":"PC","Children":[
              {{"Text":"Samsung SSD 990 PRO 2TB","HardwareId":"/nvme/0","Children":[
                {{"Text":"Temperatures","Children":[{}]}},
                {{"Text":"Levels","Children":[{},{},{},{}]}},
                {{"Text":"Factors","Children":[{}]}}]}},
              {{"Text":"Crucial MX500","HardwareId":"/ata/1","Children":[
                {{"Text":"Levels","Children":[{}]}}]}},
              {{"Text":"Msft Virtual Disk","HardwareId":"/virtual/2","Children":[
                {{"Text":"Levels","Children":[{}]}}]}}]}}]}}"#,
            sensor("/nvme/0/temperature/0", "Temperature", "Temperature", 41.0),
            sensor("/nvme/0/level/20", "Level", "Life", 97.0),
            sensor("/nvme/0/level/25", "Level", "Available Spare", 100.0),
            sensor(
                "/nvme/0/level/26",
                "Level",
                "Available Spare Threshold",
                10.0
            ),
            sensor("/nvme/0/level/27", "Level", "Percentage Used", 3.0),
            sensor("/nvme/0/factor/24", "Factor", "Power On Hours", 1234.0),
            sensor("/ata/1/level/20", "Level", "Life", 88.0),
            sensor("/virtual/2/level/20", "Level", "Life", 100.0),
        );
        let readings = parse(body.as_bytes()).unwrap();
        assert_eq!(readings.smart.len(), 2, "virtual disks are left out");
        let nvme = &readings.smart[0];
        assert_eq!(nvme.device, "disk0");
        assert_eq!(nvme.protocol, "NVMe");
        assert_eq!(nvme.model, "Samsung SSD 990 PRO 2TB");
        assert_eq!(nvme.temperature_c, Some(41.0));
        assert_eq!(nvme.percentage_used, Some(3));
        assert_eq!(nvme.available_spare_percent, Some(100));
        assert_eq!(nvme.available_spare_threshold_percent, Some(10));
        assert_eq!(nvme.power_on_hours, Some(1234));
        assert!(!nvme.needs_attention());
        let sata = &readings.smart[1];
        assert_eq!(
            (sata.device.as_str(), sata.protocol.as_str()),
            ("disk1", "ATA")
        );
        assert_eq!(sata.percentage_used, Some(12), "100 - life");
        // Without a plain "Temperature", the first numbered sensor; never the
        // warning or critical thresholds.
        let mut out = LhmReadings::default();
        add_storage_sensor(&mut out, "SSD", "nvme", "3", "Warning Temperature", 80.0);
        add_storage_sensor(&mut out, "SSD", "nvme", "3", "Temperature #1", 38.0);
        add_storage_sensor(&mut out, "SSD", "nvme", "3", "Temperature #2", 45.0);
        assert_eq!(out.smart[0].temperature_c, Some(38.0));
        // The drive temperature still shows as a sensor chip too.
        assert!(readings.chips.iter().any(|c| c.key == "lhm:/nvme/0"));
    }

    #[test]
    fn older_versions_without_ids_or_raw_values() {
        // No HardwareId / SensorId / Type / RawValue, and a decimal comma.
        let body = r#"{"Text":"Sensor","Children":[{"Text":"PC","Children":[
          {"Text":"Nuvoton NCT6796D","Children":[
            {"Text":"Temperatures","Children":[{"Text":"CPU Core","Value":"41,5 °C","Children":[]}]},
            {"Text":"Fans","Children":[{"Text":"Fan #1","Value":"1200 RPM","Children":[]}]}]}]}]}"#;
        let readings = parse(body.as_bytes()).unwrap();
        assert_eq!(readings.chips.len(), 1);
        let chip = &readings.chips[0];
        assert_eq!(chip.key, "lhm:Nuvoton NCT6796D");
        assert_eq!(chip.temperatures[0].celsius, 41.5);
        assert_eq!(chip.temperatures[0].key, "lhm:Nuvoton NCT6796D/CPU Core");
        assert_eq!(chip.fans[0].rpm, 1200);
    }

    #[test]
    fn intel_labels() {
        assert_eq!(
            cpu_label("CPU Package", "/intelcpu/0").as_deref(),
            Some("Package")
        );
        assert_eq!(
            cpu_label("Core #3", "/intelcpu/0").as_deref(),
            Some("Core 3")
        );
        assert_eq!(cpu_label("Core #3 Distance to TjMax", "/intelcpu/0"), None);
        assert_eq!(cpu_label("Core Average", "/intelcpu/0"), None);
        assert_eq!(
            cpu_label("CCD2 (Tdie)", "/amdcpu/0").as_deref(),
            Some("Tccd2")
        );
    }

    /// Serves `response` once on a local port; returns the URL.
    fn serve_once(response: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            // Read the whole request: closing with unread data resets the
            // connection.
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap() > 0 && line != "\r\n" {
                line.clear();
            }
            stream.write_all(response.as_bytes()).unwrap();
        });
        format!("http://127.0.0.1:{port}/data.json")
    }

    #[test]
    fn reads_over_http_and_backs_off_when_unreachable() {
        let body = r#"{"Text":"Sensor","Children":[{"Text":"PC","Children":[{"Text":"AMD Ryzen","HardwareId":"/amdcpu/0","Children":[{"Text":"Powers","Children":[{"Text":"Package","SensorId":"/amdcpu/0/power/0","Type":"Power","Value":"20 W","RawValue":20.0,"Children":[]}]}]}]}]}"#;
        let response: &'static str = Box::leak(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .into_boxed_str(),
        );
        let mut reader = LhmReader::new(&serve_once(response));
        let now = Instant::now();
        assert_eq!(reader.read(now).unwrap().cpu_package_w, Some(20.0));

        // Authentication enabled in LHM: an error, then a pause.
        let mut reader = LhmReader::new(&serve_once("HTTP/1.1 401 Unauthorized\r\n\r\n"));
        assert!(reader.read(now).is_none());
        assert!(reader.retry_at.is_some_and(|at| at > now));
        assert!(reader.read(now + Duration::from_secs(1)).is_none());
        assert!(
            reader.problem().unwrap().contains("401"),
            "kept while waiting"
        );

        assert!(LhmReader::new("").read(now).is_none(), "disabled");
        assert!(http_get("https://localhost/data.json", TIMEOUT).is_err());
    }
}
