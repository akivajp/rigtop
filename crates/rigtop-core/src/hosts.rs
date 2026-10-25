//! The hosts a front-end monitors: this machine plus remote hosts from the
//! configuration and the command line.

use crate::config::{Config, HostConfig, OverviewSection};
use crate::demo::DemoHost;
use crate::remote::{DEFAULT_REMOTE_COMMAND, RemoteTarget};

/// Name of the entry for the machine rigtop runs on.
pub const LOCAL_NAME: &str = "local";

/// Where a host's data comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKind {
    /// This machine.
    Local,
    /// A remote host reached through SSH.
    Remote(RemoteTarget),
    /// A made-up host (`--demo`).
    Demo(DemoHost),
}

/// One monitored host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSpec {
    /// Name for tabs and cards.
    pub name: String,
    pub kind: HostKind,
    /// Overview sections in priority order (host override or the default).
    pub sections: Vec<OverviewSection>,
}

impl HostSpec {
    /// Whether this is the local machine.
    pub fn is_local(&self) -> bool {
        self.kind == HostKind::Local
    }
}

/// The local machine first, then `[[hosts]]` from the configuration, then
/// destinations from the command line that the configuration does not
/// already list (matched by name or SSH destination).
pub fn host_specs(
    config: &Config,
    extra_destinations: &[String],
    remote_command: Option<&str>,
) -> Vec<HostSpec> {
    let default_sections = config.overview.sections.clone();
    let mut specs = vec![HostSpec {
        name: LOCAL_NAME.to_owned(),
        kind: HostKind::Local,
        sections: default_sections.clone(),
    }];
    let mut hosts: Vec<HostConfig> = config.hosts.clone();
    for destination in extra_destinations {
        let known = hosts
            .iter()
            .any(|h| h.name == *destination || h.destination() == destination);
        if !known {
            let mut host = HostConfig::from_destination(destination);
            host.command = remote_command.map(str::to_owned);
            hosts.push(host);
        }
    }
    for host in hosts {
        let command = host.command.as_deref().unwrap_or(DEFAULT_REMOTE_COMMAND);
        let mut target = RemoteTarget::ssh(host.destination(), command);
        target.name = host.name.clone();
        specs.push(HostSpec {
            sections: host
                .sections
                .clone()
                .unwrap_or_else(|| default_sections.clone()),
            name: host.name,
            kind: HostKind::Remote(target),
        });
    }
    specs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_first_then_config_then_new_command_line_hosts() {
        let mut config = Config::default();
        config.hosts.push(HostConfig {
            name: "nas".to_owned(),
            ssh: Some("admin@nas.local".to_owned()),
            command: Some("/opt/rigtop --agent".to_owned()),
            sections: Some(vec![OverviewSection::Storage]),
        });
        let extra = vec!["admin@nas.local".to_owned(), "zefat".to_owned()];
        let specs = host_specs(&config, &extra, Some("~/bin/rigtop --agent"));
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        // "admin@nas.local" is the configured nas, not a new host.
        assert_eq!(names, vec!["local", "nas", "zefat"]);
        assert!(specs[0].is_local());
        assert_eq!(specs[1].sections, vec![OverviewSection::Storage]);
        assert_eq!(specs[2].sections, OverviewSection::ALL.to_vec());
        match (&specs[1].kind, &specs[2].kind) {
            (HostKind::Remote(nas), HostKind::Remote(zefat)) => {
                assert_eq!(nas.name, "nas");
                assert!(nas.args.ends_with(&[
                    "admin@nas.local".to_owned(),
                    "/opt/rigtop --agent".to_owned()
                ]));
                // Command-line hosts use --remote-command.
                assert!(
                    zefat
                        .args
                        .ends_with(&["zefat".to_owned(), "~/bin/rigtop --agent".to_owned()])
                );
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
