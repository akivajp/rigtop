//! Linux md (mdadm) arrays from `/proc/mdstat`.
//!
//! Example of the format:
//!
//! ```text
//! Personalities : [raid1] [raid5]
//! md1 : active raid5 sdc1[3](S) sdd1[2] sde1[1](F) sdf1[0]
//!       1953260544 blocks super 1.2 level 5, 512k chunk, algorithm 2 [3/2] [U_U]
//!       [=>...................]  recovery =  8.5% (83046144/976630272) finish=79.2min speed=187904K/sec
//!
//! unused devices: <none>
//! ```

use std::fs;

use super::SysRoots;
use crate::snapshot::{MdArray, MdMember, MdSync};

/// Background operations that report progress.
const SYNC_ACTIONS: &[&str] = &["resync", "recovery", "reshape", "check"];

/// Reads `/proc/mdstat`. Returns an empty list when md is not loaded.
pub fn read_arrays(roots: &SysRoots) -> Vec<MdArray> {
    fs::read_to_string(roots.proc.join("mdstat"))
        .map(|text| parse(&text))
        .unwrap_or_default()
}

/// Parses `/proc/mdstat` text.
pub(crate) fn parse(text: &str) -> Vec<MdArray> {
    let mut arrays: Vec<MdArray> = Vec::new();
    for line in text.lines() {
        if let Some(array) = parse_header(line) {
            arrays.push(array);
        } else if line.starts_with(char::is_whitespace) {
            // Indented lines continue the most recent array.
            if let Some(array) = arrays.last_mut() {
                parse_detail(line, array);
            }
        }
    }
    arrays
}

/// Parses `md1 : active raid5 sdc1[3](S) ...`. Returns `None` for other lines
/// (`Personalities`, `unused devices`, details).
fn parse_header(line: &str) -> Option<MdArray> {
    let (name, rest) = line.split_once(" : ")?;
    if !name.starts_with("md") {
        return None;
    }
    let mut array = MdArray {
        name: name.trim().to_owned(),
        ..Default::default()
    };
    for token in rest.split_whitespace() {
        match token {
            "active" => array.active = true,
            "inactive" => array.active = false,
            "(read-only)" | "(auto-read-only)" => array.read_only = true,
            _ if token.contains('[') => {
                if let Some(member) = parse_member(token) {
                    array.members.push(member);
                }
            }
            // The only other bare word on an active array is the level.
            _ if array.level.is_empty() => array.level = token.to_owned(),
            _ => {}
        }
    }
    Some(array)
}

/// Parses a member token such as `sde1[1](F)`.
fn parse_member(token: &str) -> Option<MdMember> {
    let (name, rest) = token.split_once('[')?;
    let (role, flags) = rest.split_once(']')?;
    Some(MdMember {
        name: name.to_owned(),
        role: role.parse().ok()?,
        faulty: flags.contains("(F)"),
        spare: flags.contains("(S)"),
    })
}

/// Picks the device counts and sync progress out of a detail line.
fn parse_detail(line: &str, array: &mut MdArray) {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    for (i, token) in tokens.iter().enumerate() {
        // "[3/2]": expected/working device counts.
        if let Some(inner) = token.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
            if let Some((expected, working)) = inner.split_once('/')
                && let (Ok(e), Ok(w)) = (expected.parse(), working.parse())
            {
                array.expected_devices = Some(e);
                array.working_devices = Some(w);
            }
            continue;
        }
        // "recovery = 8.5%": the action name, a lone "=", then the percentage.
        if SYNC_ACTIONS.contains(token) && tokens.get(i + 1) == Some(&"=") {
            let percent = tokens
                .get(i + 2)
                .and_then(|p| p.strip_suffix('%'))
                .and_then(|p| p.parse().ok());
            if let Some(percent) = percent {
                array.sync = Some(MdSync {
                    action: (*token).to_owned(),
                    percent,
                    ..Default::default()
                });
            }
            continue;
        }
        if let Some(sync) = array.sync.as_mut() {
            if let Some(minutes) = token
                .strip_prefix("finish=")
                .and_then(|v| v.strip_suffix("min"))
            {
                sync.finish_minutes = minutes.parse().ok();
            } else if let Some(speed) = token
                .strip_prefix("speed=")
                .and_then(|v| v.strip_suffix("K/sec"))
            {
                sync.speed_kib_per_sec = speed.parse().ok();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
Personalities : [raid1] [raid6] [raid5] [raid4]
md127 : active (auto-read-only) raid1 sdb1[1] sda1[0]
      976630464 blocks super 1.2 [2/2] [UU]
      bitmap: 0/8 pages [0KB], 65536KB chunk

md1 : active raid5 sdc1[3](S) sdd1[2] sde1[1](F) sdf1[0]
      1953260544 blocks super 1.2 level 5, 512k chunk, algorithm 2 [3/2] [U_U]
      [=>...................]  recovery =  8.5% (83046144/976630272) finish=79.2min speed=187904K/sec

md2 : inactive sdg1[0](S)
      976630464 blocks super 1.2

unused devices: <none>
";

    #[test]
    fn parses_healthy_degraded_and_inactive_arrays() {
        let arrays = parse(SAMPLE);
        assert_eq!(arrays.len(), 3);

        let md127 = &arrays[0];
        assert_eq!(md127.name, "md127");
        assert!(md127.active && md127.read_only);
        assert_eq!(md127.level, "raid1");
        assert_eq!(md127.members.len(), 2);
        assert_eq!(
            (md127.expected_devices, md127.working_devices),
            (Some(2), Some(2))
        );
        assert!(!md127.is_degraded());
        assert!(md127.sync.is_none());

        let md1 = &arrays[1];
        assert_eq!(md1.level, "raid5");
        assert!(md1.members.iter().any(|m| m.name == "sde1" && m.faulty));
        assert!(
            md1.members
                .iter()
                .any(|m| m.name == "sdc1" && m.spare && m.role == 3)
        );
        assert!(md1.is_degraded());
        let sync = md1.sync.as_ref().unwrap();
        assert_eq!(sync.action, "recovery");
        assert!((sync.percent - 8.5).abs() < 1e-6);
        assert_eq!(sync.finish_minutes, Some(79.2));
        assert_eq!(sync.speed_kib_per_sec, Some(187904));

        let md2 = &arrays[2];
        assert!(!md2.active);
        assert!(md2.level.is_empty());
        assert_eq!(md2.members.len(), 1);
    }

    #[test]
    fn empty_mdstat_has_no_arrays() {
        assert!(parse("Personalities : \nunused devices: <none>\n").is_empty());
    }
}
