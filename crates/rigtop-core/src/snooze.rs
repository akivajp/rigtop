//! Alerts silenced for a while: snoozed for an hour or a day, or until they
//! clear. A snoozed alert drops out of status dots, notifications and the
//! `--watch` command, and comes back (notified anew) when its time is up.
//!
//! The list is one JSON file in the state directory shared by every
//! front-end, so an alert snoozed in the GUI is also quiet in `--watch`.
//! Each front-end reads it again when it changes.

use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::view::{Alert, alert_key};

/// How long to snooze an alert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnoozeFor {
    Hour,
    Day,
    /// Until the alert goes away (then the snooze ends by itself).
    UntilCleared,
}

impl SnoozeFor {
    /// Every choice, shortest first.
    pub const ALL: [SnoozeFor; 3] = [SnoozeFor::Hour, SnoozeFor::Day, SnoozeFor::UntilCleared];

    /// Short label for buttons and status lines.
    pub fn label(self) -> &'static str {
        match self {
            SnoozeFor::Hour => "1 hour",
            SnoozeFor::Day => "1 day",
            SnoozeFor::UntilCleared => "until it clears",
        }
    }

    fn until_ms(self, now_ms: u64) -> Option<u64> {
        match self {
            SnoozeFor::Hour => Some(now_ms + 3_600_000),
            SnoozeFor::Day => Some(now_ms + 86_400_000),
            SnoozeFor::UntilCleared => None,
        }
    }
}

/// One snoozed alert of one host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snooze {
    pub host: String,
    /// [`alert_key`] of the alert, so that "92% full" and "93% full" are
    /// the same alert.
    pub key: String,
    /// The alert's text when it was snoozed, for lists.
    pub text: String,
    /// When the snooze ends (milliseconds since the Unix epoch); `None`
    /// until the alert clears.
    pub until_ms: Option<u64>,
}

impl Snooze {
    /// Whether it still holds at `now_ms`.
    fn holds(&self, now_ms: u64) -> bool {
        self.until_ms.is_none_or(|until| now_ms < until)
    }
}

/// The snoozed alerts, kept in a file when made [`Snoozes::persistent`].
#[derive(Debug, Default)]
pub struct Snoozes {
    entries: Vec<Snooze>,
    file: Option<PathBuf>,
    /// Modification time and size of the file when last read or written.
    seen: Option<(SystemTime, u64)>,
}

/// The file every front-end shares: next to the alert histories.
pub fn snooze_path() -> Option<PathBuf> {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .map(|dir| dir.join("rigtop").join("snoozed.json"))
}

impl Snoozes {
    /// The snoozes kept in `path` (none when it is missing or unreadable).
    pub fn persistent(path: PathBuf) -> Self {
        let mut snoozes = Self {
            file: Some(path),
            ..Self::default()
        };
        snoozes.read();
        snoozes
    }

    /// Reads the file again if another front-end changed it.
    pub fn reload(&mut self) {
        if self.file.is_some() && self.stamp() != self.seen {
            self.read();
        }
    }

    /// Snoozes `alert` of `host` for `duration` from `now_ms`.
    pub fn snooze(&mut self, host: &str, alert: &Alert, duration: SnoozeFor, now_ms: u64) {
        self.reload();
        let key = alert_key(alert);
        self.entries.retain(|s| !(s.host == host && s.key == key));
        self.entries.push(Snooze {
            host: host.to_owned(),
            key,
            text: alert.text.clone(),
            until_ms: duration.until_ms(now_ms),
        });
        self.write();
    }

    /// Ends the snooze of `alert` of `host`, if any.
    pub fn unsnooze(&mut self, host: &str, alert: &Alert) {
        self.reload();
        let key = alert_key(alert);
        let before = self.entries.len();
        self.entries.retain(|s| !(s.host == host && s.key == key));
        if self.entries.len() != before {
            self.write();
        }
    }

    /// The snooze holding `alert` of `host` at `now_ms`, if any.
    pub fn get(&self, host: &str, alert: &Alert, now_ms: u64) -> Option<&Snooze> {
        let key = alert_key(alert);
        self.entries
            .iter()
            .find(|s| s.host == host && s.key == key && s.holds(now_ms))
    }

    /// The [`alert_key`]s of `host`'s alerts snoozed at `now_ms`.
    pub fn keys(&self, host: &str, now_ms: u64) -> Vec<String> {
        self.entries
            .iter()
            .filter(|s| s.host == host && s.holds(now_ms))
            .map(|s| s.key.clone())
            .collect()
    }

    /// Splits the alerts of `host` into those to show and those snoozed.
    pub fn split(&self, host: &str, alerts: Vec<Alert>, now_ms: u64) -> (Vec<Alert>, Vec<Alert>) {
        alerts
            .into_iter()
            .partition(|a| self.get(host, a, now_ms).is_none())
    }

    /// Ends the snoozes of `host` that are over: past their time, or
    /// "until it clears" with the alert no longer among `alerts` (all of
    /// the host's current alerts, snoozed or not).
    pub fn prune(&mut self, host: &str, alerts: &[Alert], now_ms: u64) {
        let present: Vec<String> = alerts.iter().map(alert_key).collect();
        let before = self.entries.len();
        self.entries.retain(|s| {
            s.host != host
                || match s.until_ms {
                    Some(until) => now_ms < until,
                    None => present.contains(&s.key),
                }
        });
        if self.entries.len() != before {
            self.write();
        }
    }

    /// Every snooze, in the order they were made.
    pub fn entries(&self) -> &[Snooze] {
        &self.entries
    }

    fn stamp(&self) -> Option<(SystemTime, u64)> {
        let metadata = fs::metadata(self.file.as_ref()?).ok()?;
        Some((metadata.modified().ok()?, metadata.len()))
    }

    fn read(&mut self) {
        let Some(path) = &self.file else {
            return;
        };
        self.entries = fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        self.seen = self.stamp();
    }

    /// Saves the list (through a temporary file, so that a front-end never
    /// reads half of it). Failing leaves the snoozes working in memory.
    fn write(&mut self) {
        let Some(path) = &self.file else {
            return;
        };
        let Ok(text) = serde_json::to_string_pretty(&self.entries) else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("json.tmp");
        if fs::write(&tmp, text).is_ok() {
            let _ = fs::rename(&tmp, path);
        }
        self.seen = self.stamp();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::Severity;

    fn alert(text: &str) -> Alert {
        Alert {
            severity: Severity::Warning,
            text: text.to_owned(),
        }
    }

    #[test]
    fn snoozed_alerts_drop_out_until_their_time_or_clearing() {
        let mut snoozes = Snoozes::default();
        snoozes.snooze("nas", &alert("/srv 92% full"), SnoozeFor::Hour, 0);
        snoozes.snooze(
            "nas",
            &alert("sdc: SMART warning"),
            SnoozeFor::UntilCleared,
            0,
        );
        let current = vec![
            alert("/srv 93% full"),
            alert("sdc: SMART warning"),
            alert("I/O pressure 40%"),
        ];
        let (shown, quiet) = snoozes.split("nas", current.clone(), 1_000);
        assert_eq!(shown, [alert("I/O pressure 40%")]);
        assert_eq!(quiet.len(), 2, "the number changed, the alert did not");
        // Another host's alert of the same text is not snoozed.
        assert_eq!(snoozes.split("zefat", current.clone(), 1_000).0.len(), 3);

        // An hour later the first is back.
        let (shown, _) = snoozes.split("nas", current.clone(), 3_600_000);
        assert_eq!(shown.len(), 2);
        snoozes.prune("nas", &current, 3_600_000);
        assert_eq!(snoozes.entries().len(), 1);

        // "Until it clears" ends once the alert is gone.
        snoozes.prune("nas", &[alert("I/O pressure 40%")], 3_700_000);
        assert!(snoozes.entries().is_empty());
    }

    #[test]
    fn front_ends_share_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rigtop/snoozed.json");
        let mut gui = Snoozes::persistent(path.clone());
        let mut watch = Snoozes::persistent(path.clone());
        gui.snooze("nas", &alert("md0 degraded"), SnoozeFor::Day, 0);
        watch.reload();
        assert!(watch.get("nas", &alert("md0 degraded"), 1).is_some());
        watch.unsnooze("nas", &alert("md0 degraded"));
        gui.reload();
        assert!(gui.entries().is_empty());
        // A broken file reads as nothing snoozed.
        fs::write(&path, "not json").unwrap();
        assert!(Snoozes::persistent(path).entries().is_empty());
    }
}
