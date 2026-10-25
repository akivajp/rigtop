//! Thread counts from `/proc/<pid>/stat`.
//!
//! `sysinfo` only knows a process's threads when it refreshes every thread
//! as well (`/proc/<pid>/task/*`), which costs several reads per thread on
//! every sample: on a desktop with thousands of browser and Electron
//! threads, that is most of the collection time. The count itself is one
//! field of the process's own `stat` file.

use std::fs;

use super::SysRoots;

/// Number of threads of `pid`, or `None` if it is gone or unreadable.
pub fn thread_count(roots: &SysRoots, pid: u32) -> Option<u32> {
    let text = fs::read_to_string(roots.proc.join(pid.to_string()).join("stat")).ok()?;
    parse_thread_count(&text)
}

/// Field 20 (`num_threads`) of a `stat` line. The command name (field 2) is
/// in parentheses and may itself contain spaces and parentheses, so fields
/// are counted from the last `)`.
pub(crate) fn parse_thread_count(stat: &str) -> Option<u32> {
    let after_name = &stat[stat.rfind(')')? + 1..];
    // Field 3 (state) is the first one after the name, so field 20 is the 18th.
    after_name.split_whitespace().nth(17)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::testutil::write;

    #[test]
    fn counts_threads_even_with_odd_command_names() {
        let stat = "4242 (Web Content (x)) S 1 4242 4242 0 -1 4194560 100 0 0 0 \
                    50 20 0 0 20 0 37 0 1234 567890 1000 18446744073709551615";
        assert_eq!(parse_thread_count(stat), Some(37));
        assert_eq!(parse_thread_count("1 (init) S 1"), None, "truncated");
        assert_eq!(parse_thread_count("garbage"), None);
    }

    #[test]
    fn reads_from_proc() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "proc/7/stat",
            "7 (rigtop) R 1 7 7 0 -1 0 0 0 0 0 0 0 0 0 20 0 3 0 1 1 1\n",
        );
        let roots = SysRoots {
            sys: tmp.path().join("sys"),
            proc: tmp.path().join("proc"),
        };
        assert_eq!(thread_count(&roots, 7), Some(3));
        assert_eq!(thread_count(&roots, 8), None);
    }
}
