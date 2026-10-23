//! Mount table from `/proc/mounts`.

use std::fs;

use super::SysRoots;

/// One line of the mount table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountEntry {
    /// Source device (e.g. `/dev/nvme1n1p1`).
    pub device: String,
    /// Mount point.
    pub mount_point: String,
    /// Filesystem type.
    pub fs_type: String,
}

/// Reads `/proc/mounts`. Returns an empty list if it cannot be read.
pub fn read_mounts(roots: &SysRoots) -> Vec<MountEntry> {
    fs::read_to_string(roots.proc.join("mounts"))
        .map(|text| parse(&text))
        .unwrap_or_default()
}

/// Parses mount table text.
pub(crate) fn parse(text: &str) -> Vec<MountEntry> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some(MountEntry {
                device: unescape(fields.next()?),
                mount_point: unescape(fields.next()?),
                fs_type: fields.next()?.to_owned(),
            })
        })
        .collect()
}

/// Decodes the octal escapes (`\040` for space, etc.) used in the mount table.
fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // An escape is a backslash followed by exactly three octal digits.
        if bytes[i] == b'\\' && i + 4 <= bytes.len() {
            let octal = std::str::from_utf8(&bytes[i + 1..i + 4]).ok();
            if let Some(value) = octal.and_then(|o| u8::from_str_radix(o, 8).ok()) {
                out.push(value);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fields_and_escapes() {
        let entries = parse(
            "/dev/nvme1n1p1 / btrfs rw,relatime 0 0\n/dev/sda1 /mnt/my\\040disk ext4 rw 0 0\n",
        );
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].fs_type, "btrfs");
        assert_eq!(entries[1].mount_point, "/mnt/my disk");
    }
}
