//! systemd units in the "failed" state, from
//! `systemctl list-units --state=failed` (no root needed), refreshed at
//! most every [`FAILED_UNITS_REFRESH`] to keep the cost of spawning it
//! negligible.

use std::process::Command;
use std::time::{Duration, Instant};

/// How often the failed units are looked up again.
const FAILED_UNITS_REFRESH: Duration = Duration::from_secs(30);

/// Failed units of the system manager, cached between refreshes.
#[derive(Debug, Default)]
pub struct FailedUnitsReader {
    last: Option<(Instant, Vec<String>)>,
}

impl FailedUnitsReader {
    /// Creates a reader with an empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// The failed units (e.g. `"nginx.service"`); empty without systemd.
    pub fn read(&mut self, now: Instant) -> Vec<String> {
        if let Some((at, units)) = &self.last
            && now.saturating_duration_since(*at) < FAILED_UNITS_REFRESH
        {
            return units.clone();
        }
        let units = Command::new("systemctl")
            .args([
                "list-units",
                "--state=failed",
                "--plain",
                "--no-legend",
                "--no-pager",
            ])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| parse(&String::from_utf8_lossy(&out.stdout)))
            .unwrap_or_default();
        self.last = Some((now, units.clone()));
        units
    }
}

/// Unit names from `systemctl list-units --plain --no-legend` output: the
/// first word of each line.
pub(crate) fn parse(text: &str) -> Vec<String> {
    text.lines()
        // Older versions mark failed units with "●" even with --plain.
        .map(|line| line.trim_start().trim_start_matches('●'))
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unit_names() {
        let text = "nginx.service loaded failed failed A high performance web server\n\
                    ● backup.timer  loaded failed failed Nightly backup\n\n";
        assert_eq!(parse(text), vec!["nginx.service", "backup.timer"]);
        assert!(parse("").is_empty());
    }
}
