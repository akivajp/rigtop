//! Desktop notifications when a new alert appears on a host (a drive
//! failing, a RAID device missing, a filesystem almost full...), so problems
//! surface even while the window is in the background.
//!
//! Not on Windows yet: notify-rust shows Windows toasts under PowerShell's
//! app id, and that got the unsigned executable flagged by Defender.

use std::collections::{HashMap, HashSet};
#[cfg(not(windows))]
use std::thread;

use rigtop_core::config::NotifyLevel;
use rigtop_core::view::{Alert, Severity, alert_key as key};

/// Remembers each host's alerts to announce only new ones.
pub struct Notifier {
    level: NotifyLevel,
    /// Alert keys per host at its previous check.
    seen: HashMap<String, HashSet<String>>,
}

impl Notifier {
    pub fn new(level: NotifyLevel) -> Self {
        Self {
            level,
            seen: HashMap::new(),
        }
    }

    /// The alerts of `host` worth announcing that it did not have at the
    /// previous check. The first check of a host only records its alerts:
    /// those are on screen when rigtop starts.
    pub fn new_alerts(&mut self, host: &str, alerts: &[Alert]) -> Vec<Alert> {
        let min = match self.level {
            NotifyLevel::Off => return Vec::new(),
            NotifyLevel::Warning => Severity::Warning,
            NotifyLevel::Critical => Severity::Critical,
        };
        let wanted: Vec<&Alert> = alerts.iter().filter(|a| a.severity >= min).collect();
        let current: HashSet<String> = wanted.iter().map(|a| key(a)).collect();
        let fresh = match self.seen.get(host) {
            Some(previous) => wanted
                .into_iter()
                .filter(|a| !previous.contains(&key(a)))
                .cloned()
                .collect(),
            None => Vec::new(),
        };
        self.seen.insert(host.to_owned(), current);
        fresh
    }
}

/// The notification `--test-notification` shows.
pub fn send_test() {
    send(
        "rigtop",
        &Alert {
            severity: Severity::Warning,
            text: "Test notification: alerts will appear like this.".to_owned(),
        },
    );
}

/// Whether this build can show desktop notifications.
pub const SUPPORTED: bool = cfg!(not(windows));

/// Shows a notification without blocking the UI (the notification service
/// may be slow or missing; errors are ignored). Does nothing on Windows.
#[cfg(not(windows))]
pub fn send(host: &str, alert: &Alert) {
    let summary = match alert.severity {
        Severity::Critical => format!("rigtop: {host} needs attention"),
        Severity::Warning => format!("rigtop: {host}"),
    };
    let body = alert.text.clone();
    thread::spawn(move || {
        let _ = notify_rust::Notification::new()
            .appname("rigtop")
            .summary(&summary)
            .body(&body)
            .show();
    });
}

#[cfg(windows)]
pub fn send(_host: &str, _alert: &Alert) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn alert(severity: Severity, text: &str) -> Alert {
        Alert {
            severity,
            text: text.to_owned(),
        }
    }

    #[test]
    fn announces_only_new_alerts_after_the_first_check() {
        let mut notifier = Notifier::new(NotifyLevel::Critical);
        let full = alert(Severity::Critical, "/srv 98% full");
        // Already there at startup: on screen, not announced.
        assert!(
            notifier
                .new_alerts("nas", std::slice::from_ref(&full))
                .is_empty()
        );

        let errors = alert(Severity::Critical, "btrfs /: 3 I/O errors (nvme0n1)");
        let fresh = notifier.new_alerts("nas", &[full.clone(), errors.clone()]);
        assert_eq!(fresh, vec![errors]);

        // Counts and percentages changing are the same alert.
        let more = alert(Severity::Critical, "btrfs /: 4 I/O errors (nvme0n1)");
        let fuller = alert(Severity::Critical, "/srv 99% full");
        assert!(notifier.new_alerts("nas", &[fuller, more]).is_empty());

        // Gone and back: announced again. Other hosts are separate.
        assert!(notifier.new_alerts("nas", &[]).is_empty());
        assert_eq!(
            notifier
                .new_alerts("nas", std::slice::from_ref(&full))
                .len(),
            1
        );
        assert!(
            notifier
                .new_alerts("zion", std::slice::from_ref(&full))
                .is_empty()
        );
    }

    #[test]
    fn level_filters_by_severity() {
        let warm = alert(Severity::Warning, "GPU 0 at 86°C");
        let mut critical_only = Notifier::new(NotifyLevel::Critical);
        critical_only.new_alerts("h", &[]);
        assert!(
            critical_only
                .new_alerts("h", std::slice::from_ref(&warm))
                .is_empty()
        );

        let mut warnings = Notifier::new(NotifyLevel::Warning);
        warnings.new_alerts("h", &[]);
        assert_eq!(
            warnings.new_alerts("h", std::slice::from_ref(&warm)).len(),
            1
        );

        // A warning turning critical (90% -> 97% full) is announced.
        let mut notifier = Notifier::new(NotifyLevel::Warning);
        notifier.new_alerts("h", &[alert(Severity::Warning, "/ 91% full")]);
        let critical = alert(Severity::Critical, "/ 97% full");
        assert_eq!(
            notifier
                .new_alerts("h", std::slice::from_ref(&critical))
                .len(),
            1
        );

        let mut off = Notifier::new(NotifyLevel::Off);
        off.new_alerts("h", &[]);
        assert!(off.new_alerts("h", &[critical]).is_empty());
    }
}
