//! Which container a process runs in, from its cgroup
//! (`/proc/<pid>/cgroup`, no root needed).
//!
//! Docker, Podman and Kubernetes (containerd, CRI-O) name their cgroups
//! after the container id; names come from `docker ps` / `podman ps`, run
//! on a background thread only when an unknown id appears (and at most
//! every [`NAME_LOOKUP_INTERVAL`]), so a slow or stuck daemon never holds up
//! a sample. Without a name, the short id is shown. LXC and systemd-nspawn
//! put the container's name in the cgroup itself.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use super::SysRoots;
use crate::snapshot::ProcessInfo;

/// Least time between two runs of `docker ps` / `podman ps`.
const NAME_LOOKUP_INTERVAL: Duration = Duration::from_secs(30);

/// Length of a short container id, as `docker ps` shows it.
const SHORT_ID: usize = 12;

/// A container as its cgroup names it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum ContainerRef {
    /// A container id (Docker, Podman, Kubernetes), named by a lookup.
    Id(String),
    /// A container whose cgroup carries its name (LXC, systemd-nspawn).
    Named(String),
}

/// The container of the process whose `/proc/<pid>/cgroup` is `text`, if
/// any. Looks at every hierarchy (cgroup v1 lists several) and at each path
/// from its end, since containers nest cgroups of their own inside.
pub(crate) fn parse_cgroup(text: &str) -> Option<ContainerRef> {
    text.lines()
        .filter_map(|line| line.splitn(3, ':').nth(2))
        .find_map(|path| path.rsplit('/').find_map(parse_component))
}

/// One component of a cgroup path.
fn parse_component(part: &str) -> Option<ContainerRef> {
    // Docker (systemd driver), Podman, containerd and CRI-O under systemd.
    // "libpod-conmon-<id>.scope" is Podman's monitor, outside the container.
    for prefix in ["docker-", "libpod-", "cri-containerd-", "crio-"] {
        if let Some(id) = part
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_suffix(".scope"))
            .filter(|id| is_container_id(id))
        {
            return Some(ContainerRef::Id(id.to_owned()));
        }
    }
    // Docker with the cgroupfs driver: "/docker/<id>".
    if is_container_id(part) {
        return Some(ContainerRef::Id(part.to_owned()));
    }
    if let Some(name) = part.strip_prefix("lxc.payload.") {
        return Some(ContainerRef::Named(name.to_owned()));
    }
    part.strip_prefix("systemd-nspawn@")
        .and_then(|rest| rest.strip_suffix(".service"))
        .map(|name| ContainerRef::Named(name.to_owned()))
}

/// Container ids are 64 hex digits.
fn is_container_id(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Fills in [`ProcessInfo::container`], remembering each process's
/// container between samples.
#[derive(Debug, Default)]
pub struct ContainerTracker {
    /// Per pid: the process name when its cgroup was read (a different name
    /// means the pid was reused) and its container.
    by_pid: HashMap<u32, (String, Option<ContainerRef>)>,
    /// Container names by full id.
    names: HashMap<String, String>,
    /// Ids already looked up without finding a name.
    unnamed: HashSet<String>,
    /// When names were last looked up.
    last_lookup: Option<Instant>,
    /// A lookup running in the background.
    pending: Option<Receiver<HashMap<String, String>>>,
}

impl ContainerTracker {
    /// Creates a tracker that knows no containers yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the container of each process.
    pub fn annotate(&mut self, roots: &SysRoots, processes: &mut [ProcessInfo], now: Instant) {
        self.receive_names();
        let alive: HashSet<u32> = processes.iter().map(|p| p.pid).collect();
        self.by_pid.retain(|pid, _| alive.contains(pid));

        let mut unknown = false;
        for process in processes.iter_mut() {
            let cached = self
                .by_pid
                .get(&process.pid)
                .filter(|(name, _)| *name == process.name);
            let container = match cached {
                Some((_, container)) => container.clone(),
                None => {
                    let path = roots.proc.join(process.pid.to_string()).join("cgroup");
                    let container = fs::read_to_string(path)
                        .ok()
                        .and_then(|text| parse_cgroup(&text));
                    self.by_pid
                        .insert(process.pid, (process.name.clone(), container.clone()));
                    container
                }
            };
            process.container = container.map(|c| match c {
                ContainerRef::Named(name) => name,
                ContainerRef::Id(id) => match self.names.get(&id) {
                    Some(name) => name.clone(),
                    None => {
                        unknown |= !self.unnamed.contains(&id);
                        id[..SHORT_ID].to_owned()
                    }
                },
            });
        }
        if unknown {
            self.start_lookup(now);
        }
    }

    /// Takes the names of a finished lookup.
    fn receive_names(&mut self) {
        let Some(rx) = &self.pending else {
            return;
        };
        let names = match rx.try_recv() {
            Ok(names) => names,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pending = None;
                return;
            }
        };
        self.pending = None;
        // Ids seen so far that the lookup did not name stay short ids until
        // a new container appears.
        let ids = self.by_pid.values().filter_map(|(_, c)| match c {
            Some(ContainerRef::Id(id)) => Some(id.clone()),
            _ => None,
        });
        self.unnamed = ids.filter(|id| !names.contains_key(id)).collect();
        self.names = names;
    }

    /// Looks names up in the background, unless one is running or ran less
    /// than [`NAME_LOOKUP_INTERVAL`] ago.
    fn start_lookup(&mut self, now: Instant) {
        let recent = self
            .last_lookup
            .is_some_and(|at| now.saturating_duration_since(at) < NAME_LOOKUP_INTERVAL);
        if self.pending.is_some() || recent {
            return;
        }
        self.last_lookup = Some(now);
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        thread::spawn(move || {
            let mut names = HashMap::new();
            for program in ["docker", "podman"] {
                names.extend(list_containers(program));
            }
            let _ = tx.send(names);
        });
    }
}

/// Running containers as `<program> ps` lists them: full id -> name.
/// Empty when the program is missing or not allowed (e.g. not in the
/// `docker` group).
fn list_containers(program: &str) -> HashMap<String, String> {
    Command::new(program)
        .args(["ps", "--no-trunc", "--format", "{{.ID}} {{.Names}}"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| parse_ps(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or_default()
}

/// "<id> <name>" lines into id -> name.
pub(crate) fn parse_ps(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|line| line.trim().split_once(' '))
        .filter(|(id, _)| is_container_id(id))
        .map(|(id, name)| (id.to_owned(), name.trim().to_owned()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::testutil::write;

    const ID: &str = "1712446b193ad552c0d920be599af9ba2bf571a05315a64c1be25fd712e1f17f";

    #[test]
    fn recognizes_container_cgroups() {
        let id = |s: &str| Some(ContainerRef::Id(s.to_owned()));
        let cases = [
            (format!("0::/system.slice/docker-{ID}.scope"), id(ID)),
            (format!("0::/docker/{ID}"), id(ID)),
            (
                format!(
                    "0::/user.slice/user-1000.slice/user@1000.service/user.slice/libpod-{ID}.scope/container"
                ),
                id(ID),
            ),
            (
                format!(
                    "0::/kubepods.slice/kubepods-burstable.slice/kubepods-burstable-pod1.slice/cri-containerd-{ID}.scope"
                ),
                id(ID),
            ),
            // cgroup v1: several hierarchies.
            (
                format!("12:pids:/docker/{ID}\n11:cpu,cpuacct:/docker/{ID}\n0::/"),
                id(ID),
            ),
            (
                "0::/lxc.payload.web/system.slice/nginx.service".to_owned(),
                Some(ContainerRef::Named("web".to_owned())),
            ),
            (
                "0::/machine.slice/systemd-nspawn@build.service/payload".to_owned(),
                Some(ContainerRef::Named("build".to_owned())),
            ),
        ];
        for (text, expected) in cases {
            assert_eq!(parse_cgroup(&text), expected, "{text}");
        }
        for text in [
            "0::/system.slice/containerd.service".to_owned(),
            "0::/user.slice/user-1000.slice/session-2.scope".to_owned(),
            format!(
                "0::/user.slice/user-1000.slice/user@1000.service/user.slice/libpod-conmon-{ID}.scope"
            ),
            "0::/".to_owned(),
        ] {
            assert_eq!(parse_cgroup(&text), None, "{text}");
        }
    }

    #[test]
    fn parses_ps_output() {
        let text = format!("{ID} web-frontend-1\nshort nope\n\n");
        let names = parse_ps(&text);
        assert_eq!(names.len(), 1);
        assert_eq!(names[ID], "web-frontend-1");
    }

    #[test]
    fn annotates_processes_and_remembers_them() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "proc/10/cgroup",
            &format!("0::/system.slice/docker-{ID}.scope\n"),
        );
        write(
            tmp.path(),
            "proc/11/cgroup",
            "0::/user.slice/session-2.scope\n",
        );
        write(tmp.path(), "proc/12/cgroup", "0::/lxc.payload.web\n");
        let roots = SysRoots {
            sys: tmp.path().join("sys"),
            proc: tmp.path().join("proc"),
        };
        let process = |pid, name: &str| ProcessInfo {
            pid,
            name: name.to_owned(),
            ..Default::default()
        };
        let mut processes = vec![
            process(10, "nginx"),
            process(11, "bash"),
            process(12, "init"),
        ];
        let mut tracker = ContainerTracker::new();
        // A lookup is pending: no name yet, so the short id.
        tracker.pending = Some(mpsc::channel().1);
        tracker.last_lookup = Some(Instant::now());
        tracker.annotate(&roots, &mut processes, Instant::now());
        let containers: Vec<Option<&str>> =
            processes.iter().map(|p| p.container.as_deref()).collect();
        assert_eq!(containers, vec![Some(&ID[..12]), None, Some("web")]);

        // Names arrive; the cgroup is not read again for a known process.
        let (tx, rx) = mpsc::channel();
        tx.send(HashMap::from([(ID.to_owned(), "frontend".to_owned())]))
            .unwrap();
        tracker.pending = Some(rx);
        fs::remove_file(tmp.path().join("proc/10/cgroup")).unwrap();
        tracker.annotate(&roots, &mut processes, Instant::now());
        assert_eq!(processes[0].container.as_deref(), Some("frontend"));

        // A reused pid (another program) is read again.
        let mut reused = vec![process(10, "python3")];
        tracker.annotate(&roots, &mut reused, Instant::now());
        assert_eq!(reused[0].container, None);
    }
}
