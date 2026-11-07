//! UPS readings from Network UPS Tools (NUT): the plain-text protocol of its
//! `upsd` over TCP (RFC 9271), so that no extra program or privilege is
//! needed. `upsd` usually listens on 127.0.0.1:3493 of the machine the UPS
//! is plugged into.
//!
//! ```text
//! LIST UPS                       LIST VAR ups
//! BEGIN LIST UPS                 BEGIN LIST VAR ups
//! UPS ups "Test UPS"             VAR ups battery.charge "100"
//! END LIST UPS                   VAR ups ups.status "OL CHRG"
//!                                END LIST VAR ups
//! ```

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use crate::snapshot::UpsStats;

/// NUT's port.
pub const DEFAULT_PORT: u16 = 3493;
/// How long to wait for upsd before giving up on this sample.
const TIMEOUT: Duration = Duration::from_millis(800);
/// How often to ask upsd; its drivers poll the UPS every 2 seconds by default.
const POLL_EVERY: Duration = Duration::from_secs(2);
/// How long to wait before trying again when upsd was never reached (no
/// NUT on this machine, most often).
const RETRY_AFTER: Duration = Duration::from_secs(30);
/// How long to wait when upsd stopped answering after it had.
const RETRY_LOST_AFTER: Duration = Duration::from_secs(5);
/// Upper bounds against something else listening at the address.
const MAX_LINE: u64 = 4096;
const MAX_LINES: usize = 2000;

/// What a failed request ran into.
#[derive(Debug)]
enum RequestError {
    /// upsd answered with an error (`ERR DATA-STALE`).
    Nut(String),
    /// The connection failed or the answer made no sense.
    Io(String),
}

/// An open connection to upsd.
struct Connection {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl Connection {
    fn open(address: &str) -> Result<Self, String> {
        let target = address
            .to_socket_addrs()
            .map_err(|err| reason(&err))?
            .next()
            .ok_or_else(|| "no address".to_owned())?;
        let stream = TcpStream::connect_timeout(&target, TIMEOUT).map_err(|err| reason(&err))?;
        stream
            .set_read_timeout(Some(TIMEOUT))
            .and_then(|()| stream.set_write_timeout(Some(TIMEOUT)))
            .map_err(|err| reason(&err))?;
        let writer = stream.try_clone().map_err(|err| reason(&err))?;
        Ok(Self {
            reader: BufReader::new(stream),
            writer,
        })
    }

    /// Sends `command` (`LIST UPS`) and returns the lines between its
    /// `BEGIN` and `END` lines.
    fn request(&mut self, command: &str) -> Result<Vec<String>, RequestError> {
        self.writer
            .write_all(format!("{command}\n").as_bytes())
            .map_err(|err| RequestError::Io(reason(&err)))?;
        let first = self.line()?;
        if let Some(error) = first.strip_prefix("ERR ") {
            return Err(RequestError::Nut(error.trim().to_owned()));
        }
        if first != format!("BEGIN {command}") {
            return Err(RequestError::Io(format!("unexpected answer: {first}")));
        }
        let end = format!("END {command}");
        let mut lines = Vec::new();
        loop {
            let line = self.line()?;
            if line == end {
                return Ok(lines);
            }
            if lines.len() >= MAX_LINES {
                return Err(RequestError::Io("answer too long".to_owned()));
            }
            lines.push(line);
        }
    }

    /// One line of the answer, without its line break.
    fn line(&mut self) -> Result<String, RequestError> {
        let mut line = String::new();
        let read = (&mut self.reader)
            .take(MAX_LINE)
            .read_line(&mut line)
            .map_err(|err| RequestError::Io(reason(&err)))?;
        if read == 0 {
            return Err(RequestError::Io("upsd closed the connection".to_owned()));
        }
        Ok(line.trim_end_matches(['\r', '\n']).to_owned())
    }
}

/// Polls upsd, keeping one connection open, and backing off while it is
/// unreachable.
pub struct NutReader {
    /// `host:port`; empty when disabled.
    address: String,
    connection: Option<Connection>,
    /// When to ask upsd again; the last readings are returned until then.
    next_poll: Option<Instant>,
    last: Vec<UpsStats>,
}

impl NutReader {
    /// Reads from upsd at `address` ("host" or "host:port", IPv6 addresses
    /// in brackets); an empty address disables it.
    pub fn new(address: &str) -> Self {
        Self {
            address: with_port(address.trim()),
            connection: None,
            next_poll: None,
            last: Vec::new(),
        }
    }

    /// The UPSes upsd knows, asked again at most every [`POLL_EVERY`].
    /// Empty when disabled or when upsd was never reached; UPSes read before
    /// stay listed, with an error, while upsd does not answer.
    pub fn read(&mut self, now: Instant) -> Vec<UpsStats> {
        if self.address.is_empty() || self.next_poll.is_some_and(|at| now < at) {
            return self.last.clone();
        }
        match self.query() {
            Ok(list) => {
                self.last = list;
                self.next_poll = Some(now + POLL_EVERY);
            }
            Err(err) => {
                self.connection = None;
                if self.last.is_empty() {
                    self.next_poll = Some(now + RETRY_AFTER);
                } else {
                    // Known UPSes stay, without readings: losing upsd means
                    // losing the shutdown on a power failure too.
                    self.last = self
                        .last
                        .iter()
                        .map(|ups| UpsStats {
                            name: ups.name.clone(),
                            model: ups.model.clone(),
                            error: Some(format!("upsd not reachable: {err}")),
                            ..Default::default()
                        })
                        .collect();
                    self.next_poll = Some(now + RETRY_LOST_AFTER);
                }
            }
        }
        self.last.clone()
    }

    /// Asks for every UPS and its variables; a connection kept from an
    /// earlier poll may have been closed by upsd, so a failure on it is
    /// tried once more on a new one.
    fn query(&mut self) -> Result<Vec<UpsStats>, String> {
        let reused = self.connection.is_some();
        match self.query_once() {
            Err(_) if reused => {
                self.connection = None;
                self.query_once()
            }
            result => result,
        }
    }

    fn query_once(&mut self) -> Result<Vec<UpsStats>, String> {
        let connection = match &mut self.connection {
            Some(connection) => connection,
            None => self.connection.insert(Connection::open(&self.address)?),
        };
        let names = match connection.request("LIST UPS") {
            Ok(lines) => parse_list_ups(&lines),
            Err(RequestError::Nut(error)) => return Err(error),
            Err(RequestError::Io(error)) => return Err(error),
        };
        let mut list = Vec::new();
        for (name, description) in names {
            let ups = match connection.request(&format!("LIST VAR {name}")) {
                Ok(lines) => ups_from_vars(&name, &description, &parse_vars(&name, &lines)),
                // upsd is fine, this UPS is not (its driver lost it).
                Err(RequestError::Nut(error)) => UpsStats {
                    name,
                    model: (!description.is_empty()).then_some(description),
                    error: Some(error_text(&error)),
                    ..Default::default()
                },
                Err(RequestError::Io(error)) => return Err(error),
            };
            list.push(ups);
        }
        Ok(list)
    }
}

/// An I/O error for an alert: "connection refused", without the "(os
/// error 111)" that follows it.
fn reason(err: &std::io::Error) -> String {
    let text = err.to_string();
    let text = match text.rfind(" (os error ") {
        Some(at) => &text[..at],
        None => &text,
    };
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// `address` with NUT's port if it has none.
fn with_port(address: &str) -> String {
    if address.is_empty() {
        return String::new();
    }
    let has_port = match address.rsplit_once(':') {
        // "[::1]:3493", "nas:3493"; a bare IPv6 address has more colons.
        Some((host, port)) => {
            port.parse::<u16>().is_ok() && (host.ends_with(']') || !host.contains(':'))
        }
        None => false,
    };
    if has_port {
        address.to_owned()
    } else if address.contains(':') && !address.starts_with('[') {
        format!("[{address}]:{DEFAULT_PORT}")
    } else {
        format!("{address}:{DEFAULT_PORT}")
    }
}

/// upsd's error names in words: `DATA-STALE` is "data stale".
fn error_text(error: &str) -> String {
    error
        .split_whitespace()
        .next()
        .unwrap_or(error)
        .to_ascii_lowercase()
        .replace('-', " ")
}

/// Splits a line into words, a quoted one (`"Rack \"B\""`) being one word
/// without its quotes and escapes.
fn words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c == ' ' {
            chars.next();
            continue;
        }
        let mut word = String::new();
        if c == '"' {
            chars.next();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => word.extend(chars.next()),
                    '"' => break,
                    c => word.push(c),
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c == ' ' {
                    break;
                }
                word.push(c);
                chars.next();
            }
        }
        words.push(word);
    }
    words
}

/// The UPSes of a `LIST UPS` answer: name and description.
fn parse_list_ups(lines: &[String]) -> Vec<(String, String)> {
    lines
        .iter()
        .filter_map(|line| match words(line).as_slice() {
            [kind, name, rest @ ..] if kind == "UPS" => {
                Some((name.clone(), rest.first().cloned().unwrap_or_default()))
            }
            _ => None,
        })
        .collect()
}

/// The variables of a `LIST VAR <ups>` answer.
fn parse_vars(ups: &str, lines: &[String]) -> HashMap<String, String> {
    lines
        .iter()
        .filter_map(|line| match words(line).as_slice() {
            [kind, name, var, value] if kind == "VAR" && name == ups => {
                Some((var.clone(), value.clone()))
            }
            _ => None,
        })
        .collect()
}

/// What rigtop shows of a UPS's variables.
fn ups_from_vars(name: &str, description: &str, vars: &HashMap<String, String>) -> UpsStats {
    let number = |key: &str| vars.get(key).and_then(|v| v.trim().parse::<f32>().ok());
    let text = |key: &str| {
        vars.get(key)
            .map(|v| v.trim())
            .filter(|v| !v.is_empty() && !v.eq_ignore_ascii_case("unknown"))
    };
    // "APC Back-UPS ES 700"; models often repeat the maker's name.
    let model = match (
        text("device.mfr").or(text("ups.mfr")),
        text("device.model").or(text("ups.model")),
    ) {
        (Some(maker), Some(model)) if model.starts_with(maker) => Some(model.to_owned()),
        (Some(maker), Some(model)) => Some(format!("{maker} {model}")),
        (None, Some(model)) => Some(model.to_owned()),
        _ => (!description.is_empty()).then(|| description.to_owned()),
    };
    // Many UPSes report only the load and their nominal power.
    let power_w = number("ups.realpower").or_else(|| {
        let load = number("ups.load")?;
        let nominal = number("ups.realpower.nominal")?;
        Some(load * nominal / 100.0)
    });
    UpsStats {
        name: name.to_owned(),
        model,
        status: vars
            .get("ups.status")
            .map(|s| s.split_whitespace().map(str::to_owned).collect())
            .unwrap_or_default(),
        charge_percent: number("battery.charge"),
        runtime_secs: number("battery.runtime")
            .filter(|s| *s >= 0.0)
            .map(|s| s as u64),
        load_percent: number("ups.load"),
        power_w,
        input_voltage: number("input.voltage"),
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_owned).collect()
    }

    #[test]
    fn parses_ups_lists_and_variables() {
        assert_eq!(
            words(r#"UPS rack "Rack \"B\" UPS""#),
            ["UPS", "rack", r#"Rack "B" UPS"#]
        );
        assert_eq!(
            parse_list_ups(&lines("UPS ups \"Test UPS\"\nUPS rack \"\"")),
            [
                ("ups".to_owned(), "Test UPS".to_owned()),
                ("rack".to_owned(), String::new())
            ]
        );
        let vars = parse_vars(
            "ups",
            &lines(
                "VAR ups battery.charge \"100\"\n\
                 VAR ups battery.runtime \"1830\"\n\
                 VAR ups device.mfr \"APC\"\n\
                 VAR ups device.model \"Back-UPS ES 700\"\n\
                 VAR ups input.voltage \"230.0\"\n\
                 VAR ups ups.load \"18\"\n\
                 VAR ups ups.realpower.nominal \"405\"\n\
                 VAR ups ups.status \"OL CHRG\"\n\
                 VAR other ups.status \"OB\"",
            ),
        );
        let ups = ups_from_vars("ups", "Test UPS", &vars);
        assert_eq!(ups.model.as_deref(), Some("APC Back-UPS ES 700"));
        assert_eq!(ups.status, ["OL", "CHRG"]);
        assert_eq!(ups.charge_percent, Some(100.0));
        assert_eq!(ups.runtime_secs, Some(1830));
        assert_eq!(ups.load_percent, Some(18.0));
        // 18% of 405 W.
        assert!((ups.power_w.unwrap() - 72.9).abs() < 0.01);
        assert_eq!(ups.input_voltage, Some(230.0));

        // A model that names its maker, real power reported.
        let vars: HashMap<String, String> = [
            ("ups.mfr", "CyberPower"),
            ("ups.model", "CyberPower OR1500"),
            ("ups.realpower", "260"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        let ups = ups_from_vars("rack", "", &vars);
        assert_eq!(ups.model.as_deref(), Some("CyberPower OR1500"));
        assert_eq!(ups.power_w, Some(260.0));
    }

    #[test]
    fn addresses_get_nut_port() {
        assert_eq!(with_port(""), "");
        assert_eq!(with_port("127.0.0.1"), "127.0.0.1:3493");
        assert_eq!(with_port("nas:4000"), "nas:4000");
        assert_eq!(with_port("::1"), "[::1]:3493");
        assert_eq!(with_port("[::1]:4000"), "[::1]:4000");
        assert_eq!(error_text("DRIVER-NOT-CONNECTED"), "driver not connected");
        let refused = std::io::Error::from_raw_os_error(111);
        if cfg!(target_os = "linux") {
            assert_eq!(reason(&refused), "connection refused");
        }
    }

    /// A fake upsd answering `answers` in turn, one per request line, on one
    /// connection; returns its address.
    fn fake_upsd(answers: Vec<&'static str>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            for answer in answers {
                let mut request = String::new();
                if reader.read_line(&mut request).unwrap_or(0) == 0 {
                    return;
                }
                writer.write_all(answer.as_bytes()).unwrap();
            }
        });
        address
    }

    #[test]
    fn reads_every_ups_and_keeps_a_lost_driver_listed() {
        let address = fake_upsd(vec![
            "BEGIN LIST UPS\nUPS ups \"Test UPS\"\nUPS rack \"Rack UPS\"\nEND LIST UPS\n",
            "BEGIN LIST VAR ups\nVAR ups ups.status \"OB DISCHRG\"\n\
             VAR ups battery.charge \"72\"\nEND LIST VAR ups\n",
            "ERR DRIVER-NOT-CONNECTED\n",
        ]);
        let mut reader = NutReader::new(&address);
        let list = reader.read(Instant::now());
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].status, ["OB", "DISCHRG"]);
        assert_eq!(list[0].charge_percent, Some(72.0));
        assert_eq!(list[1].name, "rack");
        assert_eq!(list[1].model.as_deref(), Some("Rack UPS"));
        assert_eq!(list[1].error.as_deref(), Some("driver not connected"));

        // upsd goes away: the UPSes stay listed, without readings.
        let later = Instant::now() + POLL_EVERY;
        let list = reader.read(later);
        assert_eq!(list.len(), 2);
        assert!(list.iter().all(|u| u.status.is_empty()));
        assert!(
            list[0]
                .error
                .as_deref()
                .is_some_and(|e| e.starts_with("upsd not reachable: ")),
            "{list:?}"
        );
    }

    #[test]
    fn no_upsd_means_no_ups() {
        // A port nothing listens on: bind one, then free it.
        let address = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .to_string();
        let mut reader = NutReader::new(&address);
        assert!(reader.read(Instant::now()).is_empty());
        assert!(NutReader::new("").read(Instant::now()).is_empty());
    }
}
