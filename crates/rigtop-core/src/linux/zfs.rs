//! ZFS pools from the kernel module's statistics, readable without root:
//! `/proc/spl/kstat/zfs/<pool>/state` holds the pool's health, the same
//! word `zpool status` shows (`ONLINE`, `DEGRADED`, `FAULTED`, ...).

use std::fs;

use super::SysRoots;
use crate::snapshot::ZfsPool;

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
}
