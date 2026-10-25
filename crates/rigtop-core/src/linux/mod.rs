//! Linux-specific readers for `/sys` and `/proc`.
//!
//! These cover what `sysinfo` does not expose: hwmon sensors, RAPL energy
//! counters, per-device disk I/O, btrfs and mdadm state.
//!
//! Every reader takes its root directories from [`SysRoots`] instead of
//! hard-coding `/sys` and `/proc`, so tests can point them at a fixture tree.

pub mod battery;
pub mod btrfs;
pub mod diskstats;
pub mod drm;
pub mod hwmon;
pub mod mdstat;
pub mod meminfo;
pub mod mounts;
pub mod net;
pub mod procstat;
pub mod rapl;

use std::fs;
use std::path::{Path, PathBuf};

/// Root directories of the kernel pseudo filesystems.
#[derive(Debug, Clone)]
pub struct SysRoots {
    /// Normally `/sys`.
    pub sys: PathBuf,
    /// Normally `/proc`.
    pub proc: PathBuf,
}

impl Default for SysRoots {
    fn default() -> Self {
        Self {
            sys: PathBuf::from("/sys"),
            proc: PathBuf::from("/proc"),
        }
    }
}

/// Reads a sysfs attribute and trims the trailing newline.
///
/// Returns `None` on any error: sysfs attributes may disappear at any time
/// (hot-unplug) or be unreadable without root, and callers treat both as
/// "not available".
pub(crate) fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_owned())
}

/// Reads a sysfs attribute holding a single unsigned integer.
pub(crate) fn read_u64(path: &Path) -> Option<u64> {
    read_trimmed(path)?.parse().ok()
}

/// Reads a sysfs attribute holding a single signed integer.
pub(crate) fn read_i64(path: &Path) -> Option<i64> {
    read_trimmed(path)?.parse().ok()
}

/// Returns the file names in `dir`, sorted, or an empty list if it cannot be read.
pub(crate) fn sorted_entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = match fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort_by(|a, b| natural_cmp(a, b));
    names
}

/// Compares strings so that embedded numbers sort numerically
/// (`hwmon2` < `hwmon10`, `fan2` < `fan10`).
pub(crate) fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    // Split into (non-digit prefix, trailing number). Kernel object names
    // ("hwmon10", "nvme2", "md127") only carry a number at the end.
    fn split(s: &str) -> (&str, Option<u64>) {
        let digits_start = s
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_ascii_digit())
            .last()
            .map(|(i, _)| i)
            .unwrap_or(s.len());
        let (prefix, digits) = s.split_at(digits_start);
        (prefix, digits.parse().ok())
    }
    let (pa, na) = split(a);
    let (pb, nb) = split(b);
    pa.cmp(pb).then(na.cmp(&nb)).then_with(|| a.cmp(b))
}

#[cfg(test)]
pub(crate) mod testutil {
    //! Helpers to build fake sysfs/procfs trees in a temporary directory.

    use std::fs;
    use std::path::Path;

    /// Writes `content` to `root/rel`, creating parent directories.
    pub fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    /// Creates a symlink at `root/rel` pointing to `target` (absolute or relative).
    pub fn symlink(root: &Path, rel: &str, target: &Path) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(target, path).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::natural_cmp;

    #[test]
    fn natural_order_sorts_numbers_numerically() {
        let mut names = vec!["hwmon10", "hwmon2", "hwmon1", "fan"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, vec!["fan", "hwmon1", "hwmon2", "hwmon10"]);
    }
}
