//! ZFS pools from the kernel module's statistics, readable without root:
//! `/proc/spl/kstat/zfs/<pool>/state` holds the pool's health, the same
//! word `zpool status` shows (`ONLINE`, `DEGRADED`, `FAULTED`, ...), and
//! `/proc/spl/kstat/zfs/arcstats` the size of the ARC, ZFS's read cache.

use std::fs;

use super::SysRoots;
use crate::snapshot::{MemoryStats, ZfsPool};

/// The ARC (adaptive replacement cache) in bytes, from `arcstats`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArcSize {
    /// What the ARC holds now.
    pub size: u64,
    /// The least it shrinks to under memory pressure (`zfs_arc_min`).
    pub min: u64,
}

impl ArcSize {
    /// What the ARC gives back when programs need the memory.
    pub fn shrinkable(&self) -> u64 {
        self.size.saturating_sub(self.min)
    }
}

/// The ARC's size, or `None` without the ZFS module.
pub fn read_arc(roots: &SysRoots) -> Option<ArcSize> {
    let text = fs::read_to_string(roots.proc.join("spl/kstat/zfs/arcstats")).ok()?;
    // A header line, a "name type data" line, then one statistic a line.
    let value = |name: &str| {
        text.lines().find_map(|line| {
            let mut words = line.split_whitespace();
            (words.next()? == name)
                .then(|| words.nth(1)?.parse::<u64>().ok())
                .flatten()
        })
    };
    Some(ArcSize {
        size: value("size")?,
        min: value("c_min").unwrap_or(0),
    })
}

/// Counts the shrinkable part of the ARC as available rather than used,
/// like page cache, and records the ARC's size: the kernel counts the ARC
/// as memory in use, so a ZFS host's memory looks full (and warns) while
/// the ARC would give it back. This is what htop and btop do.
pub fn account_arc(memory: &mut MemoryStats, arc: ArcSize) {
    let shrinkable = arc.shrinkable().min(memory.used_bytes);
    memory.used_bytes -= shrinkable;
    memory.available_bytes = memory
        .available_bytes
        .saturating_add(shrinkable)
        .min(memory.total_bytes);
    memory.zfs_arc_bytes = Some(arc.size);
}

/// Reads every imported pool. Returns an empty list without ZFS.
pub fn read_pools(roots: &SysRoots) -> Vec<ZfsPool> {
    let Ok(entries) = fs::read_dir(roots.proc.join("spl/kstat/zfs")) else {
        return Vec::new();
    };
    let mut pools: Vec<ZfsPool> = entries
        .flatten()
        .filter_map(|entry| {
            // Pools are directories with a `state` file; other entries
            // (`arcstats`, `dbufstats`, ...) are module-wide statistics.
            let state = fs::read_to_string(entry.path().join("state")).ok()?;
            Some(ZfsPool {
                name: entry.file_name().to_string_lossy().into_owned(),
                state: state.trim().to_owned(),
                ..Default::default()
            })
        })
        .collect();
    pools.sort_by(|a, b| a.name.cmp(&b.name));
    pools
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_pool_states_and_skips_module_statistics() {
        let tmp = tempfile::tempdir().unwrap();
        let kstat = tmp.path().join("proc/spl/kstat/zfs");
        for (pool, state) in [("tank", "ONLINE\n"), ("backup", "DEGRADED\n")] {
            fs::create_dir_all(kstat.join(pool)).unwrap();
            fs::write(kstat.join(pool).join("state"), state).unwrap();
        }
        fs::write(kstat.join("arcstats"), "13 1 0x01 ...\n").unwrap();
        let roots = SysRoots {
            sys: tmp.path().join("sys"),
            proc: tmp.path().join("proc"),
        };
        let pools = read_pools(&roots);
        let found: Vec<(&str, &str)> = pools
            .iter()
            .map(|p| (p.name.as_str(), p.state.as_str()))
            .collect();
        assert_eq!(found, vec![("backup", "DEGRADED"), ("tank", "ONLINE")]);
        assert!(pools[0].is_failing());
        assert!(pools[1].is_online());

        let none = SysRoots {
            sys: tmp.path().join("nowhere"),
            proc: tmp.path().join("nowhere"),
        };
        assert!(read_pools(&none).is_empty());
    }

    #[test]
    fn the_shrinkable_arc_counts_as_available() {
        let tmp = tempfile::tempdir().unwrap();
        let kstat = tmp.path().join("proc/spl/kstat/zfs");
        fs::create_dir_all(&kstat).unwrap();
        // As OpenZFS 2.2 writes it (shortened).
        fs::write(
            kstat.join("arcstats"),
            "13 1 0x01 147 39984 4296727063 2041361617349\n\
             name                            type data\n\
             hits                            4    1209012\n\
             misses                          4    20031\n\
             c                               4    1056718848\n\
             c_min                           4    67108864\n\
             c_max                           4    1056718848\n\
             size                            4    805306368\n\
             compressed_size                 4    700000000\n",
        )
        .unwrap();
        let roots = SysRoots {
            sys: tmp.path().join("sys"),
            proc: tmp.path().join("proc"),
        };
        let arc = read_arc(&roots).unwrap();
        assert_eq!(
            arc,
            ArcSize {
                size: 768 << 20,
                min: 64 << 20
            }
        );

        let gib = 1u64 << 30;
        let mut memory = MemoryStats {
            total_bytes: 2 * gib,
            used_bytes: gib + (512 << 20),
            available_bytes: 512 << 20,
            ..Default::default()
        };
        account_arc(&mut memory, arc);
        // 704 MiB of the 768 MiB ARC can go; 64 MiB stays (zfs_arc_min).
        assert_eq!(memory.used_bytes, gib + (512 << 20) - (704 << 20));
        assert_eq!(memory.available_bytes, (512 << 20) + (704 << 20));
        assert_eq!(memory.zfs_arc_bytes, Some(768 << 20));

        assert_eq!(
            read_arc(&SysRoots {
                sys: tmp.path().join("nowhere"),
                proc: tmp.path().join("nowhere"),
            }),
            None
        );
    }
}
