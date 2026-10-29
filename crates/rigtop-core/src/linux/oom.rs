//! Processes stopped by the kernel's OOM killer, from the `oom_kill`
//! counter of `/proc/vmstat` (no root needed; it counts system-wide and
//! cgroup OOM kills, not those of userspace killers such as systemd-oomd).
//!
//! The kernel log that names the process needs root, so the likely one is
//! the largest process that went away in the same interval, if it was
//! large enough to be one ([`LIKELY_MIN_BYTES`]): one that started and was
//! stopped between two samples is not known, and a small process that
//! merely exited would be a wrong guess.

use std::collections::{HashMap, HashSet};
use std::fs;

use super::SysRoots;
use crate::snapshot::{OomKill, ProcessInfo};

/// How long a kill stays in the snapshot (and so in the alerts).
pub const OOM_KILL_WINDOW_MS: u64 = 60 * 60 * 1000;

/// Smallest process taken for the one the OOM killer stopped.
const LIKELY_MIN_BYTES: u64 = 64 << 20;

/// Counts OOM kills between samples.
#[derive(Debug, Default)]
pub struct OomTracker {
    /// The counter at the previous sample.
    last_count: Option<u64>,
    /// Memory of each process at the previous sample: pid -> (name, bytes).
    previous: HashMap<u32, (String, u64)>,
    /// Kills seen within [`OOM_KILL_WINDOW_MS`], oldest first.
    kills: Vec<OomKill>,
}

impl OomTracker {
    /// Creates a tracker; the first sample only sets the starting count.
    pub fn new() -> Self {
        Self::default()
    }

    /// Kills seen in the last [`OOM_KILL_WINDOW_MS`] up to `now_ms`, given
    /// the processes of this sample.
    pub fn sample(
        &mut self,
        roots: &SysRoots,
        processes: &[ProcessInfo],
        now_ms: u64,
    ) -> Vec<OomKill> {
        let count = fs::read_to_string(roots.proc.join("vmstat"))
            .ok()
            .and_then(|text| parse_oom_kill(&text));
        if let (Some(count), Some(last)) = (count, self.last_count) {
            let new = count.saturating_sub(last);
            if new > 0 {
                let likely = likely_victims(&self.previous, processes, new as usize);
                for i in 0..new as usize {
                    self.kills.push(OomKill {
                        at_ms: now_ms,
                        likely_process: likely.get(i).cloned(),
                    });
                }
            }
        }
        if count.is_some() {
            self.last_count = count;
        }
        self.previous = processes
            .iter()
            .map(|p| (p.pid, (p.name.clone(), p.memory_bytes)))
            .collect();
        self.kills
            .retain(|k| now_ms.saturating_sub(k.at_ms) < OOM_KILL_WINDOW_MS);
        self.kills.clone()
    }
}

/// The value of the `oom_kill` line of `/proc/vmstat` text.
pub(crate) fn parse_oom_kill(text: &str) -> Option<u64> {
    text.lines()
        .find_map(|line| line.strip_prefix("oom_kill "))
        .and_then(|value| value.trim().parse().ok())
}

/// Names of the `count` largest processes of `previous` missing from
/// `current`, largest first.
fn likely_victims(
    previous: &HashMap<u32, (String, u64)>,
    current: &[ProcessInfo],
    count: usize,
) -> Vec<String> {
    let alive: HashSet<u32> = current.iter().map(|p| p.pid).collect();
    let mut gone: Vec<&(String, u64)> = previous
        .iter()
        .filter(|(pid, (_, bytes))| !alive.contains(pid) && *bytes >= LIKELY_MIN_BYTES)
        .map(|(_, process)| process)
        .collect();
    gone.sort_by_key(|(_, bytes)| std::cmp::Reverse(*bytes));
    gone.into_iter()
        .take(count)
        .map(|(name, _)| name.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::testutil::write;

    fn process(pid: u32, name: &str, gib: u64) -> ProcessInfo {
        ProcessInfo {
            pid,
            name: name.to_owned(),
            memory_bytes: gib << 30,
            ..Default::default()
        }
    }

    #[test]
    fn parses_the_counter() {
        let text = "pgfault 123\noom_kill 4\nnr_free_pages 5\n";
        assert_eq!(parse_oom_kill(text), Some(4));
        // Kernels before 4.13 have no such line.
        assert_eq!(parse_oom_kill("pgfault 123\n"), None);
    }

    #[test]
    fn records_kills_with_the_likely_process_and_forgets_them() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = SysRoots {
            sys: tmp.path().join("sys"),
            proc: tmp.path().join("proc"),
        };
        let mut tracker = OomTracker::new();
        let before = vec![
            process(1, "systemd", 0),
            process(10, "python3", 60),
            process(11, "bash", 0),
        ];

        // Kills before rigtop started are not reported.
        write(tmp.path(), "proc/vmstat", "oom_kill 3\n");
        assert!(tracker.sample(&roots, &before, 1_000).is_empty());

        // The large python3 and the small bash went away; two kills, but
        // bash is too small to be the other one.
        write(tmp.path(), "proc/vmstat", "oom_kill 5\n");
        let after = vec![process(1, "systemd", 0)];
        let kills = tracker.sample(&roots, &after, 2_000);
        let kill = |name: Option<&str>| OomKill {
            at_ms: 2_000,
            likely_process: name.map(str::to_owned),
        };
        assert_eq!(kills, vec![kill(Some("python3")), kill(None)]);

        // Still reported within the window, gone after it.
        assert_eq!(tracker.sample(&roots, &after, 3_000).len(), 2);
        assert!(
            tracker
                .sample(&roots, &after, 2_000 + OOM_KILL_WINDOW_MS)
                .is_empty()
        );
    }
}
