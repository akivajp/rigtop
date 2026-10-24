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

/// Formats how long ago something happened, e.g. `"45s ago"`, `"8m ago"`, `"3d ago"`.
pub fn age(secs: u64) -> String {
    match secs {
        0..10 => "just now".to_owned(),
        10..120 => format!("{secs}s ago"),
        120..7_200 => format!("{}m ago", secs / 60),
        7_200..172_800 => format!("{}h ago", secs / 3_600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

/// Formats a link speed in Mbit/s, e.g. `"10G"`, `"2.5G"`, `"100M"`.
pub fn link_speed(mbps: f64) -> String {
    if mbps >= 1000.0 {
        let gbps = mbps / 1000.0;
        if gbps.fract().abs() < 0.05 {
            format!("{gbps:.0}G")
        } else {
            format!("{gbps:.1}G")
        }
    } else {
        format!("{mbps:.0}M")
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
    fn age_picks_a_readable_unit() {
        assert_eq!(age(3), "just now");
        assert_eq!(age(45), "45s ago");
        assert_eq!(age(8 * 60), "8m ago");
        assert_eq!(age(5 * 3600), "5h ago");
        assert_eq!(age(3 * 86_400), "3d ago");
    }

    #[test]
    fn link_speed_uses_g_and_m() {
        assert_eq!(link_speed(10_000.0), "10G");
        assert_eq!(link_speed(2_500.0), "2.5G");
        assert_eq!(link_speed(1_000.0), "1G");
        assert_eq!(link_speed(576.4), "576M");
    }

    #[test]
    fn truncate_marks_the_cut() {
        assert_eq!(truncate("rigtop", 10), "rigtop");
        assert_eq!(truncate("rigtop-gui", 6), "rigto…");
        assert_eq!(truncate("abc", 0), "");
    }
}
