//! Supplementary memory figures from `/proc/meminfo`.
//!
//! `sysinfo` covers total/used/available/free; this module only adds the page
//! cache figure that btop-style memory panels show.

use std::fs;

use super::SysRoots;

/// Returns `Cached + SReclaimable` in bytes, matching what `free` reports as
/// "buff/cache" minus buffers.
pub fn read_cached_bytes(roots: &SysRoots) -> Option<u64> {
    let text = fs::read_to_string(roots.proc.join("meminfo")).ok()?;
    parse_cached_bytes(&text)
}

/// Parses the cache figure out of `/proc/meminfo` text.
pub(crate) fn parse_cached_bytes(text: &str) -> Option<u64> {
    let mut cached = None;
    let mut reclaimable = 0;
    for line in text.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        // Values are in KiB: "Cached:         12345678 kB".
        let kib = || rest.split_whitespace().next()?.parse::<u64>().ok();
        match key {
            "Cached" => cached = kib(),
            "SReclaimable" => reclaimable = kib().unwrap_or(0),
            _ => {}
        }
    }
    cached.map(|c| (c + reclaimable) * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_includes_reclaimable_slab() {
        let text = "MemTotal: 100 kB\nCached:  1000 kB\nSwapCached: 5 kB\nSReclaimable: 24 kB\n";
        assert_eq!(parse_cached_bytes(text), Some(1024 * 1024));
    }

    #[test]
    fn missing_cached_line_is_none() {
        assert_eq!(parse_cached_bytes("MemTotal: 100 kB\n"), None);
    }
}
