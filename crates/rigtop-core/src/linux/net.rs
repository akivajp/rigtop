//! Network link state from `/sys/class/net/<iface>` and, for Wi-Fi, `iw`.
//!
//! Wired links report their negotiated speed in `speed` (Mbit/s, `-1` or an
//! error when unknown). Wireless drivers do not; their current bit rates come
//! from nl80211, which is read here through `iw dev <iface> link` at most
//! every [`WIRELESS_REFRESH`] to keep the cost of spawning it negligible.

use std::collections::HashMap;
use std::process::Command;
use std::time::{Duration, Instant};

use super::{SysRoots, read_i64, read_trimmed};
use crate::snapshot::{NetworkStats, WirelessLink};

/// How often Wi-Fi bit rates are refreshed.
const WIRELESS_REFRESH: Duration = Duration::from_secs(10);

/// Adds link information to network statistics, caching Wi-Fi queries.
#[derive(Debug, Default)]
pub struct NetLinkReader {
    /// Last Wi-Fi query per interface: when it ran and what it returned.
    wireless: HashMap<String, (Instant, Option<WirelessLink>)>,
}

impl NetLinkReader {
    /// Creates a reader with an empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Fills `link_speed_mbps`, `link_up` and `wireless` of each interface.
    pub fn annotate(&mut self, roots: &SysRoots, networks: &mut [NetworkStats], now: Instant) {
        for net in networks.iter_mut() {
            let dir = roots.sys.join("class/net").join(&net.name);
            net.link_speed_mbps = read_i64(&dir.join("speed"))
                .filter(|s| *s > 0)
                .and_then(|s| u32::try_from(s).ok());
            net.link_up =
                read_trimmed(&dir.join("operstate")).and_then(|state| match state.as_str() {
                    "up" => Some(true),
                    "down" | "lowerlayerdown" | "dormant" | "notpresent" => Some(false),
                    // "unknown" is common for tunnels and loopback.
                    _ => None,
                });
            if dir.join("wireless").is_dir() {
                net.wireless = self.wireless_link(&net.name, now);
            }
        }
        // Forget interfaces that disappeared (USB adapters, VPNs).
        self.wireless
            .retain(|name, _| networks.iter().any(|n| &n.name == name));
    }

    fn wireless_link(&mut self, name: &str, now: Instant) -> Option<WirelessLink> {
        if let Some((at, link)) = self.wireless.get(name)
            && now.saturating_duration_since(*at) < WIRELESS_REFRESH
        {
            return link.clone();
        }
        let link = Command::new("iw")
            .args(["dev", name, "link"])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .and_then(|out| parse_iw_link(&String::from_utf8_lossy(&out.stdout)));
        self.wireless.insert(name.to_owned(), (now, link.clone()));
        link
    }
}

/// Parses `iw dev <iface> link` output. Returns `None` when not connected.
pub(crate) fn parse_iw_link(text: &str) -> Option<WirelessLink> {
    if !text.trim_start().starts_with("Connected") {
        return None;
    }
    let mut link = WirelessLink::default();
    for line in text.lines() {
        let Some((key, value)) = line.trim().split_once(':') else {
            continue;
        };
        // Values look like "576.4 MBit/s 160MHz HE-MCS 5 ..." or "-74 dBm".
        let first = value.split_whitespace().next();
        match key {
            "rx bitrate" => link.rx_bitrate_mbps = first.and_then(|v| v.parse().ok()),
            "tx bitrate" => link.tx_bitrate_mbps = first.and_then(|v| v.parse().ok()),
            "signal" => link.signal_dbm = first.and_then(|v| v.parse().ok()),
            _ => {}
        }
    }
    Some(link)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::testutil::write;

    const IW_CONNECTED: &str = "\
Connected to 04:42:1a:e4:c4:34 (on wlp7s0)
\tSSID: example
\tfreq: 5500.0
\tsignal: -74 dBm
\trx bitrate: 576.4 MBit/s 160MHz HE-MCS 5 HE-NSS 1 HE-GI 0 HE-DCM 0
\ttx bitrate: 490.0 MBit/s 160MHz HE-MCS 3 HE-NSS 2 HE-GI 2 HE-DCM 0
";

    #[test]
    fn parses_iw_bitrates_and_signal() {
        let link = parse_iw_link(IW_CONNECTED).unwrap();
        assert_eq!(link.rx_bitrate_mbps, Some(576.4));
        assert_eq!(link.tx_bitrate_mbps, Some(490.0));
        assert_eq!(link.signal_dbm, Some(-74));
        assert!(parse_iw_link("Not connected.\n").is_none());
    }

    #[test]
    fn reads_wired_speed_and_operstate() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "sys/class/net/enp9s0/speed", "10000\n");
        write(root, "sys/class/net/enp9s0/operstate", "up\n");
        // Unplugged ports report -1 (or fail to read) and are down.
        write(root, "sys/class/net/enp8s0/speed", "-1\n");
        write(root, "sys/class/net/enp8s0/operstate", "down\n");
        write(root, "sys/class/net/tailscale0/operstate", "unknown\n");
        let roots = SysRoots {
            sys: root.join("sys"),
            proc: root.join("proc"),
        };
        let mut networks: Vec<NetworkStats> = ["enp9s0", "enp8s0", "tailscale0"]
            .into_iter()
            .map(|name| NetworkStats {
                name: name.to_owned(),
                ..Default::default()
            })
            .collect();

        NetLinkReader::new().annotate(&roots, &mut networks, Instant::now());
        assert_eq!(networks[0].link_speed_mbps, Some(10_000));
        assert_eq!(networks[0].link_up, Some(true));
        assert_eq!(networks[1].link_speed_mbps, None);
        assert_eq!(networks[1].link_up, Some(false));
        assert_eq!(networks[2].link_up, None);
        assert!(networks[0].wireless.is_none());
    }
}
