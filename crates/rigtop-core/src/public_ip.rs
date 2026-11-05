//! The host's public IPv4 address, as an outside service sees it: behind
//! NAT it is not on any interface. Looked up with the system's `curl`, in
//! the background, at most every [`PublicIpLookup::interval`], and only
//! when the user turned it on (`[network] public_ip`), as it contacts a
//! third party.

use std::net::IpAddr;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

/// Where and how often to look up the public address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicIpLookup {
    /// A URL answering with the caller's address as plain text.
    pub url: String,
    /// Time between lookups.
    pub interval: Duration,
}

/// Shortest time between lookups, whatever the configuration says.
const MIN_INTERVAL: Duration = Duration::from_secs(60);

/// Looks the public address up now and then.
pub struct PublicIpChecker {
    lookup: PublicIpLookup,
    next: Instant,
    running: Option<Receiver<Result<String, String>>>,
    address: Option<String>,
}

impl PublicIpChecker {
    /// Looks up right away, then every `lookup.interval`.
    pub fn new(lookup: PublicIpLookup) -> Self {
        Self {
            lookup,
            next: Instant::now(),
            running: None,
            address: None,
        }
    }

    /// What it is looking up.
    pub fn lookup(&self) -> &PublicIpLookup {
        &self.lookup
    }

    /// The address last found (kept when a lookup fails, e.g. offline).
    /// Call regularly: it also starts the next lookup when due.
    pub fn poll(&mut self) -> Option<&str> {
        if let Some(running) = &self.running
            && let Ok(result) = running.try_recv()
        {
            self.running = None;
            if let Ok(address) = result {
                self.address = Some(address);
            }
        }
        if self.running.is_none() && Instant::now() >= self.next {
            self.next = Instant::now() + self.lookup.interval.max(MIN_INTERVAL);
            let (tx, rx) = mpsc::channel();
            let url = self.lookup.url.clone();
            let started = thread::Builder::new()
                .name("rigtop-public-ip".to_owned())
                .spawn(move || {
                    let _ = tx.send(fetch(&url));
                });
            if started.is_ok() {
                self.running = Some(rx);
            }
        }
        self.address.as_deref()
    }
}

/// Asks `url` for this host's address.
fn fetch(url: &str) -> Result<String, String> {
    let output = crate::update::curl()
        .args(["-fsS", "--max-time", "15"])
        .arg(url)
        .output()
        .map_err(|err| format!("cannot run curl: {err}"))?;
    if !output.status.success() {
        return Err(format!("curl exited with {}", output.status));
    }
    parse_address(&String::from_utf8_lossy(&output.stdout))
        .ok_or_else(|| "no address in the answer".to_owned())
}

/// The address in a plain-text answer ("203.0.113.5\n"); `None` for
/// anything else (an error page, say).
fn parse_address(text: &str) -> Option<String> {
    let text = text.trim();
    text.parse::<IpAddr>().ok().map(|addr| addr.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_plain_addresses_only() {
        assert_eq!(
            parse_address("203.0.113.5\n").as_deref(),
            Some("203.0.113.5")
        );
        assert_eq!(
            parse_address(" 2001:db8::1 ").as_deref(),
            Some("2001:db8::1")
        );
        assert_eq!(parse_address("<html>error</html>"), None);
        assert_eq!(parse_address(""), None);
    }
}
