//! Compact human-readable formatting for narrow terminal cells.

/// Formats a byte count with a binary unit, e.g. `"1.5G"`, `"512K"`, `"0B"`.
///
/// Uses at most 5 characters so that columns stay aligned.
pub fn bytes(value: u64) -> String {
    const UNITS: &[&str] = &["B", "K", "M", "G", "T", "P"];
    let mut v = value as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value}B")
    } else if v < 10.0 {
        format!("{v:.1}{}", UNITS[unit])
    } else {
        format!("{v:.0}{}", UNITS[unit])
    }
}

/// Formats a throughput, e.g. `"3.4M/s"`.
pub fn rate(bytes_per_sec: f64) -> String {
    format!("{}/s", bytes(bytes_per_sec.max(0.0) as u64))
}

/// Formats a duration in seconds as `"3d04h"`, `"5h12m"` or `"12:34"`.
pub fn duration(secs: u64) -> String {
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let minutes = (secs % 3_600) / 60;
    if days > 0 {
        format!("{days}d{hours:02}h")
    } else if hours > 0 {
        format!("{hours}h{minutes:02}m")
    } else {
        format!("{minutes:02}:{:02}", secs % 60)
    }
}

/// Truncates `text` to at most `width` characters, marking the cut with `…`.
pub fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut out: String = text.chars().take(width - 1).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_uses_binary_units_and_short_output() {
        assert_eq!(bytes(0), "0B");
        assert_eq!(bytes(1023), "1023B");
        assert_eq!(bytes(1536), "1.5K");
        assert_eq!(bytes(20 * 1024 * 1024), "20M");
        assert_eq!(bytes(6_000_067_395_584), "5.5T");
    }

    #[test]
    fn rate_appends_per_second() {
        assert_eq!(rate(3_560_916.6), "3.4M/s");
        assert_eq!(rate(-1.0), "0B/s");
    }

    #[test]
    fn duration_picks_the_largest_units() {
        assert_eq!(duration(75), "01:15");
        assert_eq!(duration(3 * 3600 + 5 * 60), "3h05m");
        assert_eq!(duration(2 * 86_400 + 4 * 3600), "2d04h");
    }

    #[test]
    fn truncate_marks_the_cut() {
        assert_eq!(truncate("rigtop", 10), "rigtop");
        assert_eq!(truncate("rigtop-gui", 6), "rigto…");
        assert_eq!(truncate("abc", 0), "");
    }
}
