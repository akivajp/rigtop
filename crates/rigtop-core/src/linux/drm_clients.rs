//! Per-process use of AMD and Intel GPUs, from the DRM drivers' fdinfo:
//! `/proc/<pid>/fdinfo/<fd>` of a file descriptor open on `/dev/dri/*`
//! lists the engine time and memory of that DRM client (the kernel's
//! drm-usage-stats format). Only the processes of the user rigtop runs as
//! can be read; no root is needed.
//!
//! ```text
//! drm-driver:         amdgpu
//! drm-client-id:      513
//! drm-pdev:           0000:0f:00.0
//! drm-resident-vram:  68912 KiB
//! drm-engine-compute: 3658840491 ns
//! drm-engine-enc:     13758496207 ns
//! ```
//!
//! amdgpu and i915 count engine time in nanoseconds; xe counts GPU cycles
//! (`drm-cycles-<engine>`) next to the GPU's own cycle count over the same
//! time (`drm-total-cycles-<engine>`). Several descriptors, even in several
//! processes after a fork, can share one client (the same `drm-client-id`),
//! so each client is counted once.
//!
//! Finding the descriptors takes a readlink of every open file, so a
//! process's descriptors are listed when it first shows up and then every
//! [`RESCAN_EVERY`]; in between, only the fdinfo of the descriptors found
//! is read.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use super::SysRoots;
use crate::snapshot::{GpuProcess, GpuStats, ProcessInfo};

/// How often every process's open files are listed again, to find DRM
/// descriptors opened since.
const RESCAN_EVERY: Duration = Duration::from_secs(5);

/// The busy time of one engine class of a client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct EngineTime {
    /// Nanoseconds, or GPU cycles when `total` is known (xe).
    busy: u64,
    /// The GPU's cycles over the client's lifetime (xe).
    total: Option<u64>,
    /// Engines of this class (`drm-engine-capacity-<engine>`), 1 if not said.
    capacity: u32,
}

/// One DRM client, from one fdinfo file.
#[derive(Debug, Clone, PartialEq, Default)]
struct Client {
    /// The GPU's PCI address (`0000:0f:00.0`).
    pdev: String,
    id: u64,
    /// Busy time per engine class (`gfx`, `enc`, `render`, `rcs`...).
    engines: HashMap<String, EngineTime>,
    /// Device-local memory (VRAM) in bytes, when the driver reports any.
    vram_bytes: Option<u64>,
}

/// Parses an fdinfo file; `None` when it is not a DRM client's, or lacks
/// the GPU or client id.
fn parse_fdinfo(text: &str) -> Option<Client> {
    let mut driver = false;
    let mut client = Client::default();
    let mut id = None;
    // Memory per region and how good the figure is: resident first, then
    // the older drm-memory- lines (resident too), then total.
    let mut regions: HashMap<&str, (u8, u64)> = HashMap::new();
    let mut capacities: HashMap<&str, u32> = HashMap::new();
    let mut cycles_total: HashMap<&str, u64> = HashMap::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        let Some(key) = key.trim().strip_prefix("drm-") else {
            continue;
        };
        if key == "driver" {
            driver = true;
        } else if key == "pdev" {
            client.pdev = value.to_owned();
        } else if key == "client-id" {
            id = value.parse().ok();
        } else if let Some(class) = key.strip_prefix("engine-capacity-") {
            if let Ok(n) = value.parse() {
                capacities.insert(class, n);
            }
        } else if let Some(class) = key.strip_prefix("engine-") {
            // "3658840491 ns"
            if let Some(ns) = value.split_whitespace().next().and_then(|n| n.parse().ok()) {
                client.engines.entry(class.to_owned()).or_default().busy = ns;
            }
        } else if let Some(class) = key.strip_prefix("total-cycles-") {
            if let Ok(n) = value.parse() {
                cycles_total.insert(class, n);
            }
        } else if let Some(class) = key.strip_prefix("cycles-") {
            if let Ok(n) = value.parse() {
                client.engines.entry(class.to_owned()).or_default().busy = n;
            }
        } else {
            for (prefix, rank) in [("resident-", 0), ("memory-", 1), ("total-", 2)] {
                if let Some(region) = key.strip_prefix(prefix)
                    && let Some(bytes) = parse_size(value)
                    && regions.get(region).is_none_or(|(r, _)| rank < *r)
                {
                    regions.insert(region, (rank, bytes));
                }
            }
        }
    }
    if !driver || client.pdev.is_empty() {
        return None;
    }
    client.id = id?;
    for (class, time) in &mut client.engines {
        time.capacity = capacities.get(class.as_str()).copied().unwrap_or(1).max(1);
        time.total = cycles_total.get(class.as_str()).copied();
    }
    // VRAM: amdgpu's "vram", xe's "vram0", i915's "local0".
    let vram: Vec<u64> = regions
        .iter()
        .filter(|(region, _)| region.starts_with("vram") || region.starts_with("local"))
        .map(|(_, (_, bytes))| *bytes)
        .collect();
    client.vram_bytes = (!vram.is_empty()).then(|| vram.iter().sum());
    Some(client)
}

/// "68912 KiB", "12 MiB", "4096" (bytes).
fn parse_size(value: &str) -> Option<u64> {
    let mut words = value.split_whitespace();
    let number: u64 = words.next()?.parse().ok()?;
    let unit = match words.next() {
        None => 1,
        Some("KiB") => 1 << 10,
        Some("MiB") => 1 << 20,
        Some("GiB") => 1 << 30,
        Some(_) => return None,
    };
    Some(number.saturating_mul(unit))
}

/// The share (0–100+) of its engines of one class a client kept busy
/// between two samples `elapsed` apart.
fn engine_percent(before: &EngineTime, now: &EngineTime, elapsed: Duration) -> f32 {
    let busy = now.busy.saturating_sub(before.busy) as f64;
    let capacity = f64::from(now.capacity.max(1));
    let span = match (before.total, now.total) {
        // GPU cycles against the GPU's cycles (xe).
        (Some(a), Some(b)) => b.saturating_sub(a) as f64,
        _ => elapsed.as_nanos() as f64,
    };
    if span <= 0.0 {
        return 0.0;
    }
    (busy / (span * capacity) * 100.0) as f32
}

/// What one process uses of one GPU in a sample.
#[derive(Debug, Default)]
struct Usage {
    vram_bytes: Option<u64>,
    /// Summed over the process's clients, per engine class.
    engines: HashMap<String, f32>,
    /// Whether any of its clients was seen in the previous sample too.
    measured: bool,
}

/// Fills in the processes of AMD and Intel GPUs, remembering descriptors
/// and engine times between samples.
#[derive(Debug, Default)]
pub struct DrmClientTracker {
    /// Per pid: the process name when its descriptors were listed (another
    /// name means the pid was reused), and its DRM descriptors.
    fds: HashMap<u32, (String, Vec<String>)>,
    last_rescan: Option<Instant>,
    /// Each client's engine times at the previous sample, and when they
    /// were read.
    previous: HashMap<(String, u64), (Instant, HashMap<String, EngineTime>)>,
}

impl DrmClientTracker {
    /// Creates a tracker that knows no processes yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the `processes` of each GPU in `gpus`, whose PCI addresses are
    /// in `pci` (same order): its VRAM, and the share of its busiest engine
    /// (3D, compute, video encode or decode, like the "GPU" column of
    /// Windows' Task Manager), known from a process's second sample.
    pub fn annotate(
        &mut self,
        roots: &SysRoots,
        processes: &[ProcessInfo],
        gpus: &mut [GpuStats],
        pci: &[String],
        now: Instant,
    ) {
        self.annotate_with_clock(roots, processes, gpus, pci, now, Instant::now);
    }

    /// [`DrmClientTracker::annotate`], with the clock that times each
    /// fdinfo read. Shares are over the time between two reads of a client,
    /// not between two samples: a sample can spend a while before it gets
    /// here (the first one most), which would understate them.
    fn annotate_with_clock(
        &mut self,
        roots: &SysRoots,
        processes: &[ProcessInfo],
        gpus: &mut [GpuStats],
        pci: &[String],
        now: Instant,
        mut clock: impl FnMut() -> Instant,
    ) {
        if gpus.is_empty() {
            return;
        }
        let rescan = self
            .last_rescan
            .is_none_or(|at| now.duration_since(at) >= RESCAN_EVERY);
        if rescan {
            self.last_rescan = Some(now);
        }
        let alive: HashSet<u32> = processes.iter().map(|p| p.pid).collect();
        self.fds.retain(|pid, _| alive.contains(pid));

        // In pid order, so that a client shared after a fork counts for
        // the parent (usually the lower pid).
        let mut list: Vec<&ProcessInfo> = processes.iter().filter(|p| !p.kernel_thread).collect();
        list.sort_unstable_by_key(|p| p.pid);
        let mut seen: HashSet<(String, u64)> = HashSet::new();
        let mut current = HashMap::new();
        let mut usage: HashMap<(usize, u32), Usage> = HashMap::new();
        for process in list {
            let stale = self
                .fds
                .get(&process.pid)
                .is_none_or(|(name, _)| *name != process.name);
            if rescan || stale {
                let fds = drm_fds(&roots.proc, process.pid);
                self.fds.insert(process.pid, (process.name.clone(), fds));
            }
            let Some((_, fds)) = self.fds.get_mut(&process.pid) else {
                continue;
            };
            let fdinfo = roots.proc.join(process.pid.to_string()).join("fdinfo");
            // Descriptors closed (or reused for another file) since they
            // were listed are dropped until the next listing.
            fds.retain(|fd| {
                let text = fs::read_to_string(fdinfo.join(fd));
                let read_at = clock();
                let Some(client) = text.ok().and_then(|text| parse_fdinfo(&text)) else {
                    return false;
                };
                let Some(gpu) = pci.iter().position(|p| *p == client.pdev) else {
                    return true;
                };
                let key = (client.pdev.clone(), client.id);
                if !seen.insert(key.clone()) {
                    return true;
                }
                let entry = usage.entry((gpu, process.pid)).or_default();
                if let Some(bytes) = client.vram_bytes {
                    entry.vram_bytes = Some(entry.vram_bytes.unwrap_or(0) + bytes);
                }
                if let Some((then, before)) = self.previous.get(&key) {
                    let elapsed = read_at.duration_since(*then);
                    for (class, time) in &client.engines {
                        if let Some(earlier) = before.get(class) {
                            *entry.engines.entry(class.clone()).or_default() +=
                                engine_percent(earlier, time, elapsed);
                        }
                    }
                    entry.measured = true;
                }
                current.insert(key, (read_at, client.engines));
                true
            });
        }
        self.previous = current;

        let mut per_gpu: Vec<Vec<GpuProcess>> = vec![Vec::new(); gpus.len()];
        for ((gpu, pid), usage) in usage {
            let percent = usage.measured.then(|| {
                usage
                    .engines
                    .values()
                    .copied()
                    .fold(0.0, f32::max)
                    .clamp(0.0, 100.0)
            });
            per_gpu[gpu].push(GpuProcess {
                pid,
                memory_bytes: usage.vram_bytes,
                sm_percent: percent,
            });
        }
        for (gpu, mut processes) in gpus.iter_mut().zip(per_gpu) {
            processes.sort_unstable_by_key(|p| (std::cmp::Reverse(p.memory_bytes), p.pid));
            gpu.processes = processes;
        }
    }
}

/// The descriptors of `pid` open on a DRM device (`/dev/dri/*`); none when
/// its open files cannot be read (another user's process).
fn drm_fds(proc: &Path, pid: u32) -> Vec<String> {
    let Ok(entries) = fs::read_dir(proc.join(pid.to_string()).join("fd")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| {
            fs::read_link(entry.path()).is_ok_and(|target| target.starts_with("/dev/dri"))
        })
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::testutil::{symlink, write};

    const AMDGPU: &str = "pos:\t0\nflags:\t02100002\nmnt_id:\t41\nino:\t1214\n\
        drm-driver:\tamdgpu\ndrm-client-id:\t513\ndrm-pdev:\t0000:0f:00.0\npasid:\t46\n\
        drm-total-gtt:\t4112 KiB\ndrm-resident-gtt:\t4112 KiB\n\
        drm-total-vram:\t68912 KiB\ndrm-resident-vram:\t68912 KiB\n\
        drm-memory-vram:\t68912 KiB\ndrm-memory-gtt: \t4112 KiB\n\
        drm-engine-compute:\t3658840491 ns\ndrm-engine-enc:\t13758496207 ns\n\
        amd-evicted-vram:\t0 KiB\n";

    #[test]
    fn parses_the_three_drivers() {
        let amd = parse_fdinfo(AMDGPU).unwrap();
        assert_eq!((amd.pdev.as_str(), amd.id), ("0000:0f:00.0", 513));
        assert_eq!(amd.vram_bytes, Some(68912 << 10));
        assert_eq!(amd.engines["enc"].busy, 13_758_496_207);
        assert_eq!(amd.engines["compute"].capacity, 1);
        assert_eq!(amd.engines["enc"].total, None);

        // i915 on an integrated GPU: system memory only, two video engines.
        let i915 = parse_fdinfo(
            "drm-driver:\ti915\ndrm-client-id:\t7\ndrm-pdev:\t0000:00:02.0\n\
             drm-total-system0:\t4896 KiB\ndrm-resident-system0:\t4896 KiB\n\
             drm-engine-render:\t25662044495 ns\ndrm-engine-copy:\t0 ns\n\
             drm-engine-video:\t1000 ns\ndrm-engine-capacity-video:\t2\n",
        )
        .unwrap();
        assert_eq!(i915.vram_bytes, None);
        assert_eq!(i915.engines["video"].capacity, 2);
        assert_eq!(i915.engines.len(), 3);

        // xe on a discrete card: cycles against the GPU's cycles.
        let xe = parse_fdinfo(
            "drm-driver:\txe\ndrm-client-id:\t42\ndrm-pdev:\t0000:03:00.0\n\
             drm-total-vram0:\t23992 KiB\ndrm-resident-vram0:\t20000 KiB\n\
             drm-cycles-rcs:\t28257900\ndrm-total-cycles-rcs:\t7655183225\n\
             drm-cycles-ccs:\t0\ndrm-total-cycles-ccs:\t7655183225\n\
             drm-engine-capacity-ccs:\t4\n",
        )
        .unwrap();
        assert_eq!(xe.vram_bytes, Some(20000 << 10));
        assert_eq!(
            xe.engines["rcs"],
            EngineTime {
                busy: 28_257_900,
                total: Some(7_655_183_225),
                capacity: 1
            }
        );
        assert_eq!(xe.engines["ccs"].capacity, 4);

        // Not a DRM descriptor.
        assert_eq!(parse_fdinfo("pos:\t0\nflags:\t02\nmnt_id:\t26\n"), None);
    }

    #[test]
    fn engine_shares_from_nanoseconds_and_cycles() {
        let ns = |busy, capacity| EngineTime {
            busy,
            total: None,
            capacity,
        };
        let second = Duration::from_secs(1);
        assert_eq!(engine_percent(&ns(0, 1), &ns(250_000_000, 1), second), 25.0);
        // Two video engines: half of their capacity.
        assert_eq!(
            engine_percent(&ns(0, 2), &ns(1_000_000_000, 2), second),
            50.0
        );
        // A counter that went back (a new client with the same id).
        assert_eq!(engine_percent(&ns(900, 1), &ns(100, 1), second), 0.0);
        let cycles = |busy, total| EngineTime {
            busy,
            total: Some(total),
            capacity: 1,
        };
        assert_eq!(
            engine_percent(&cycles(100, 1_000), &cycles(400, 2_000), second),
            30.0
        );
    }

    fn process(pid: u32, name: &str) -> ProcessInfo {
        ProcessInfo {
            pid,
            name: name.to_owned(),
            ..Default::default()
        }
    }

    #[test]
    fn attributes_each_client_once_to_its_gpu() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let roots = SysRoots {
            sys: root.join("sys"),
            proc: root.join("proc"),
        };
        // ffmpeg has one client on three descriptors; its child shares it
        // after a fork; a file that is not a GPU is skipped.
        let amd = |engine_ns: u64| {
            format!(
                "drm-driver:\tamdgpu\ndrm-client-id:\t513\ndrm-pdev:\t0000:0f:00.0\n\
                 drm-resident-vram:\t68912 KiB\ndrm-engine-enc:\t{engine_ns} ns\n\
                 drm-engine-compute:\t100 ns\n"
            )
        };
        let write_all = |engine_ns: u64| {
            for pid in [100, 200] {
                for fd in [3, 4, 5] {
                    write(root, &format!("proc/{pid}/fdinfo/{fd}"), &amd(engine_ns));
                }
            }
        };
        for pid in [100, 200] {
            for fd in [3, 4, 5] {
                symlink(
                    root,
                    &format!("proc/{pid}/fd/{fd}"),
                    Path::new("/dev/dri/renderD129"),
                );
            }
        }
        symlink(root, "proc/100/fd/1", Path::new("/dev/pts/3"));
        write(root, "proc/100/fdinfo/1", "pos:\t0\nflags:\t02\n");
        write_all(1_000_000_000);

        let processes = vec![process(200, "ffmpeg"), process(100, "ffmpeg")];
        let pci = vec!["0000:0f:00.0".to_owned()];
        let mut gpus = vec![GpuStats::default()];
        let mut tracker = DrmClientTracker::new();
        let start = Instant::now();
        tracker.annotate_with_clock(&roots, &processes, &mut gpus, &pci, start, || start);
        // First sample: memory, but no share yet.
        assert_eq!(gpus[0].processes.len(), 1);
        let first = &gpus[0].processes[0];
        assert_eq!((first.pid, first.memory_bytes), (100, Some(68912 << 10)));
        assert_eq!(first.sm_percent, None);

        // The next sample started a second later, but its reads came 200 ms
        // after that (a slow sample): the encoder was busy 600 ms of the
        // 1.2 s between the reads, 50%.
        write_all(1_600_000_000);
        let later = start + Duration::from_secs(1);
        let read_at = later + Duration::from_millis(200);
        tracker.annotate_with_clock(&roots, &processes, &mut gpus, &pci, later, || read_at);
        let second = &gpus[0].processes[0];
        assert_eq!(gpus[0].processes.len(), 1, "counted once");
        assert!(
            (second.sm_percent.unwrap() - 50.0).abs() < 0.01,
            "{second:?}"
        );

        // A GPU that is not listed (an NVIDIA card in NVML's hands) gets nothing.
        let mut other = vec![GpuStats::default()];
        DrmClientTracker::new().annotate(
            &roots,
            &processes,
            &mut other,
            &["0000:01:00.0".to_owned()],
            start,
        );
        assert!(other[0].processes.is_empty());
    }
}
