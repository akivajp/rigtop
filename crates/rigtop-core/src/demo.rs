//! Made-up hosts for trying the interface and taking screenshots without
//! real hardware (`rigtop --demo`, `rigtop-gui --demo`): a workstation
//! training on two GPUs, a laptop on battery and a NAS that needs attention.
//!
//! The readings follow fixed formulas of time, so every run looks the same.
//! Every poll is one second of made-up time, whatever the interval, so a
//! short interval fast-forwards (handy to fill the graphs for screenshots).

use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::Config;
use crate::hosts::{HostKind, HostSpec};
use crate::snapshot::{
    BatteryStats, BtrfsAllocation, BtrfsDeviceStats, BtrfsFilesystem, BtrfsScrub, CpuStats, DiskIo,
    Fan, FilesystemUsage, GpuProcess, GpuStats, HostInfo, MdArray, MdMember, MdSync, MemoryStats,
    NetworkStats, PowerStats, ProcessInfo, RaidStats, SensorChip, SmartDevice, Snapshot,
    Temperature, WirelessLink,
};
use crate::source::{DataSource, ProcessSignal, SourceError};

const GIB: u64 = 1 << 30;
const TIB: u64 = 1 << 40;

/// Which made-up machine a [`DemoSource`] plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemoHost {
    /// 16 cores, two NVIDIA GPUs, three NVMe drives in btrfs RAID.
    Workstation,
    /// An APU laptop on battery and Wi-Fi.
    Laptop,
    /// Four hard drives in mdadm RAID 5, resyncing, one with reallocated
    /// sectors, and a nearly full filesystem.
    Nas,
}

impl DemoHost {
    /// All demo hosts, in overview order.
    pub const ALL: [DemoHost; 3] = [DemoHost::Workstation, DemoHost::Laptop, DemoHost::Nas];

    /// Host name shown in tabs and cards.
    pub fn name(self) -> &'static str {
        match self {
            DemoHost::Workstation => "workstation",
            DemoHost::Laptop => "laptop",
            DemoHost::Nas => "nas",
        }
    }
}

/// The demo hosts as host specs, in place of this machine and `[[hosts]]`.
pub fn host_specs(config: &Config) -> Vec<HostSpec> {
    DemoHost::ALL
        .iter()
        .map(|&host| HostSpec {
            name: host.name().to_owned(),
            kind: HostKind::Demo(host),
            sections: config.overview.sections.clone(),
        })
        .collect()
}

/// A [`DataSource`] with made-up readings that change over time.
#[derive(Debug, Clone)]
pub struct DemoSource {
    host: DemoHost,
    /// Seconds of made-up time.
    time: f64,
    rx_total: f64,
    tx_total: f64,
}

impl DemoSource {
    /// A source playing `host`.
    pub fn new(host: DemoHost) -> Self {
        Self {
            host,
            time: 0.0,
            // Some traffic since boot.
            rx_total: 412.0 * GIB as f64,
            tx_total: 38.0 * GIB as f64,
        }
    }
}

impl DataSource for DemoSource {
    fn poll(&mut self) -> Result<Snapshot, SourceError> {
        self.time += 1.0;
        let mut snapshot = match self.host {
            DemoHost::Workstation => workstation(self.time),
            DemoHost::Laptop => laptop(self.time),
            DemoHost::Nas => nas(self.time),
        };
        for net in &mut snapshot.networks {
            if net.name == primary_interface(self.host) {
                self.rx_total += net.rx_bytes_per_sec;
                self.tx_total += net.tx_bytes_per_sec;
                net.rx_total_bytes = self.rx_total as u64;
                net.tx_total_bytes = self.tx_total as u64;
            }
        }
        snapshot.timestamp_ms = now_ms();
        snapshot.process_count = snapshot.processes.len();
        Ok(snapshot)
    }

    fn signal_process(&mut self, _pid: u32, _signal: ProcessSignal) -> Result<(), SourceError> {
        // Nothing to stop: pretend it worked.
        Ok(())
    }
}

fn primary_interface(host: DemoHost) -> &'static str {
    match host {
        DemoHost::Workstation => "enp9s0",
        DemoHost::Laptop => "wlp1s0",
        DemoHost::Nas => "eno1",
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// A smooth, repeatable wobble in 0..1 for series `seed` at time `t`, with
/// changes over roughly `period` seconds.
fn wobble(seed: u64, t: f64, period: f64) -> f64 {
    let x = t / period;
    let (cell, frac) = (x.floor(), x.fract());
    let at = |c: f64| hash(seed ^ (c as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    // Smoothstep between random values at whole cells.
    let s = frac * frac * (3.0 - 2.0 * frac);
    at(cell) * (1.0 - s) + at(cell + 1.0) * s
}

/// A pseudo-random value in 0..1 (splitmix64).
fn hash(seed: u64) -> f64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

/// 0..1 while a job that runs `busy` seconds out of every `period` is on,
/// with soft edges.
fn duty(t: f64, period: f64, busy: f64) -> f64 {
    let phase = t.rem_euclid(period);
    let ramp = 4.0;
    if phase < ramp {
        phase / ramp
    } else if phase < busy {
        1.0
    } else if phase < busy + ramp {
        1.0 - (phase - busy) / ramp
    } else {
        0.0
    }
}

fn temp(chip: &str, n: u32, label: &str, celsius: f64) -> Temperature {
    Temperature {
        key: format!("{chip}/temp{n}"),
        label: label.to_owned(),
        celsius: celsius as f32,
        ..Default::default()
    }
}

fn fan(chip: &str, n: u32, rpm: f64) -> Fan {
    Fan {
        key: format!("{chip}/fan{n}"),
        label: format!("fan{n}"),
        rpm: rpm as u32,
    }
}

/// Per-core usage around `total`, some cores busier than others.
fn cores(seed: u64, t: f64, count: usize, total: f64) -> Vec<f32> {
    (0..count)
        .map(|i| {
            let spread = wobble(seed + i as u64, t, 3.0 + (i % 5) as f64);
            ((total * (0.55 + 0.9 * spread)) + 2.0 * spread).clamp(0.0, 100.0) as f32
        })
        .collect()
}

/// A process; `cpu` in percent of one core.
#[allow(clippy::too_many_arguments)]
fn process(
    pid: u32,
    name: &str,
    command: &str,
    user: &str,
    cpu: f64,
    memory: u64,
    threads: u32,
    run_time_secs: u64,
) -> ProcessInfo {
    ProcessInfo {
        pid,
        parent_pid: Some(1),
        name: name.to_owned(),
        command: command.to_owned(),
        user: user.to_owned(),
        state: if cpu > 1.0 { "R" } else { "S" }.to_owned(),
        cpu_percent: cpu.max(0.0) as f32,
        memory_bytes: memory,
        threads: Some(threads),
        run_time_secs,
        ..Default::default()
    }
}

/// Gives a process disk throughput (bytes/s).
fn with_io(process: ProcessInfo, read: f64, write: f64) -> ProcessInfo {
    ProcessInfo {
        read_bytes_per_sec: read.max(0.0),
        write_bytes_per_sec: write.max(0.0),
        ..process
    }
}

fn workstation(t: f64) -> Snapshot {
    // A build every two minutes on top of a steady training run.
    let build = duty(t, 120.0, 45.0);
    let cpu_total = 9.0 + 70.0 * build + 6.0 * wobble(1, t, 4.0);
    let per_core = cores(100, t, 32, cpu_total);
    let tctl = 48.0 + 0.38 * cpu_total + 2.0 * wobble(2, t, 6.0);
    let cpu_w = 38.0 + 1.6 * cpu_total;
    // The training run dips while it evaluates every 5 minutes.
    let eval = duty(t + 200.0, 300.0, 40.0);
    let gpu0_util = 97.0 - 60.0 * eval + 3.0 * wobble(3, t, 2.0);
    let gpu0_w = 120.0 + 3.0 * gpu0_util;
    // Inference requests come and go.
    let requests = (wobble(4, t, 9.0) - 0.35).max(0.0) / 0.65;
    let gpu1_util = 85.0 * requests;
    let gpu1_w = 22.0 + 1.1 * gpu1_util;
    let download = (wobble(5, t, 15.0) - 0.4).max(0.0) / 0.6;
    let k10 = "k10temp:0000:00:18.3";
    let nct = "nct6686:nct6683.2592";
    let train_pid = 48_211;
    // The training run reads its dataset, less while evaluating.
    let train_read = (45e6 + 15e6 * wobble(9, t, 3.0)) * (1.0 - 0.8 * eval);
    let ollama_pid = 2_204;
    let mut processes = vec![
        with_io(
            process(
                train_pid,
                "python",
                "python train.py --config configs/vit-large.yaml",
                "demo",
                160.0 + 40.0 * wobble(6, t, 3.0) - 80.0 * eval,
                18 * GIB + GIB / 3,
                64,
                5 * 3600 + t as u64,
            ),
            train_read,
            0.2e6,
        ),
        process(
            ollama_pid,
            "ollama",
            "/usr/local/bin/ollama serve",
            "ollama",
            4.0 + 60.0 * requests,
            3 * GIB,
            42,
            9 * 86_400,
        ),
        process(
            3_120,
            "gnome-shell",
            "/usr/bin/gnome-shell",
            "demo",
            3.0 + 2.0 * wobble(7, t, 2.0),
            GIB,
            28,
            9 * 86_400,
        ),
        // Saves the download the network panel shows.
        with_io(
            process(
                5_310,
                "firefox",
                "/usr/lib/firefox/firefox",
                "demo",
                2.0 + 6.0 * wobble(8, t, 5.0),
                2 * GIB + GIB / 2,
                112,
                2 * 86_400,
            ),
            0.0,
            40e6 * download,
        ),
        process(
            6_002,
            "code",
            "/usr/share/code/code",
            "demo",
            1.0 + 3.0 * wobble(9, t, 4.0),
            GIB + GIB / 4,
            40,
            86_400,
        ),
        process(
            7_734,
            "rigtop-gui",
            "rigtop-gui",
            "demo",
            2.0,
            180 << 20,
            12,
            3_600,
        ),
        process(
            1_402,
            "postgres",
            "postgres: checkpointer",
            "postgres",
            0.3,
            420 << 20,
            1,
            9 * 86_400,
        ),
        process(
            1_388,
            "dockerd",
            "/usr/bin/dockerd -H fd://",
            "root",
            0.4,
            96 << 20,
            30,
            9 * 86_400,
        ),
        process(
            1_105,
            "sshd",
            "sshd: /usr/sbin/sshd -D",
            "root",
            0.0,
            12 << 20,
            1,
            9 * 86_400,
        ),
        process(
            1,
            "systemd",
            "/sbin/init",
            "root",
            0.1,
            14 << 20,
            1,
            9 * 86_400,
        ),
    ];
    // Background services and desktop programs, mostly idle.
    let idle: [(u32, &str, &str, &str, u64, u32); 22] = [
        (
            2_301,
            "jupyter-lab",
            "python -m jupyterlab --no-browser",
            "demo",
            310,
            6,
        ),
        (
            2_388,
            "tensorboard",
            "python -m tensorboard.main --logdir runs",
            "demo",
            520,
            18,
        ),
        (
            3_044,
            "Xwayland",
            "/usr/bin/Xwayland :0 -rootless",
            "demo",
            110,
            9,
        ),
        (3_090, "pipewire", "/usr/bin/pipewire", "demo", 24, 3),
        (3_092, "wireplumber", "/usr/bin/wireplumber", "demo", 30, 6),
        (
            3_401,
            "tmux: server",
            "tmux new-session -s work",
            "demo",
            9,
            1,
        ),
        (3_420, "zsh", "-zsh", "demo", 7, 1),
        (
            3_520,
            "node",
            "node /usr/lib/node_modules/vite/bin/vite.js",
            "demo",
            260,
            11,
        ),
        (3_610, "claude", "claude", "demo", 330, 19),
        (1_201, "containerd", "/usr/bin/containerd", "root", 52, 18),
        (
            1_266,
            "redis-server",
            "/usr/bin/redis-server 127.0.0.1:6379",
            "redis",
            18,
            5,
        ),
        (1_270, "nginx", "nginx: worker process", "www-data", 11, 1),
        (
            512,
            "systemd-journal",
            "/usr/lib/systemd/systemd-journald",
            "root",
            64,
            1,
        ),
        (
            880,
            "NetworkManager",
            "/usr/sbin/NetworkManager --no-daemon",
            "root",
            21,
            4,
        ),
        (902, "tailscaled", "/usr/sbin/tailscaled", "root", 48, 14),
        (
            930,
            "nvidia-persiste",
            "/usr/bin/nvidia-persistenced",
            "nvidia-persistenced",
            2,
            1,
        ),
        (1_010, "cupsd", "/usr/sbin/cupsd -l", "root", 8, 1),
        (1_050, "smartd", "/usr/sbin/smartd -n", "root", 6, 1),
        (
            3_150,
            "gnome-terminal-",
            "/usr/libexec/gnome-terminal-server",
            "demo",
            90,
            5,
        ),
        (
            3_180,
            "nautilus",
            "/usr/bin/nautilus --gapplication-service",
            "demo",
            140,
            9,
        ),
        (
            3_230,
            "evolution-alarm",
            "/usr/libexec/evolution-data-server/evolution-alarm-notify",
            "demo",
            70,
            7,
        ),
        (
            3_260,
            "ibus-daemon",
            "/usr/bin/ibus-daemon --panel disable",
            "demo",
            15,
            4,
        ),
    ];
    for (i, &(pid, name, command, user, mib, threads)) in idle.iter().enumerate() {
        let cpu = (wobble(200 + i as u64, t, 4.0) - 0.6).max(0.0) * 2.0;
        processes.push(process(
            pid,
            name,
            command,
            user,
            cpu,
            mib << 20,
            threads,
            9 * 86_400 - pid as u64 * 7,
        ));
    }
    if build > 0.0 {
        // cargo and a few rustc jobs while building.
        processes.push(process(
            60_100,
            "cargo",
            "cargo build --release",
            "demo",
            1.0,
            200 << 20,
            8,
            40,
        ));
        for i in 0..6 {
            processes.push(with_io(
                process(
                    60_120 + i,
                    "rustc",
                    "rustc --crate-name rigtop_core --edition=2024",
                    "demo",
                    (90.0 + 10.0 * wobble(10 + i as u64, t, 2.0)) * build,
                    (600 + 100 * i as u64) << 20,
                    2,
                    20,
                ),
                3e6 * build,
                12e6 * build * wobble(40 + i as u64, t, 2.0),
            ));
        }
    }
    let nvme = |i: u32, celsius: f64| SensorChip {
        key: format!("nvme:nvme{i}"),
        name: "nvme".to_owned(),
        temperatures: vec![temp(&format!("nvme:nvme{i}"), 1, "Composite", celsius)],
        fans: Vec::new(),
    };
    let drive_temp = |i: u32| 44.0 + 4.0 * i as f64 + 5.0 * build;
    // The processes' reads and writes, spread over the three drives of the
    // RAID0 (the build also reads sources and writes objects).
    let disk_io = |i: u64| {
        let busy = build * (0.5 + 0.5 * wobble(20 + i, t, 2.0));
        DiskIo {
            name: format!("nvme{i}n1"),
            read_bytes_per_sec: train_read / 3.0 + 30e6 * busy + 1e6 * wobble(30 + i, t, 3.0),
            write_bytes_per_sec: 40e6 * busy + 40e6 / 3.0 * download,
            busy_percent: Some((25.0 * busy + 3.0 * download + 4.0 * train_read / 60e6) as f32),
            temperature_c: Some(drive_temp(i as u32) as f32),
        }
    };
    let smart = |i: u32| SmartDevice {
        device: format!("nvme{i}"),
        protocol: "NVMe".to_owned(),
        model: "Samsung SSD 990 PRO 2TB".to_owned(),
        passed: Some(true),
        temperature_c: Some(drive_temp(i) as f32),
        power_on_hours: Some(4_210 + 13 * i as u64),
        percentage_used: Some(3),
        available_spare_percent: Some(100),
        available_spare_threshold_percent: Some(10),
        critical_warning: Some(0),
        media_errors: Some(0),
        data_written_bytes: Some(41 * TIB + i as u64 * TIB),
        collected_at_ms: Some(now_ms() - 140_000),
        ..Default::default()
    };
    let used_memory = 38.0 + 6.0 * build + 1.5 * wobble(11, t, 20.0);
    Snapshot {
        host: HostInfo {
            hostname: "workstation".to_owned(),
            os: "Linux (Ubuntu 26.04)".to_owned(),
            kernel: "7.0.0-38-generic".to_owned(),
            uptime_secs: 9 * 86_400 + 4 * 3600 + t as u64,
            rigtop_version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        cpu: CpuStats {
            model: "AMD Ryzen 9 9950X3D 16-Core Processor".to_owned(),
            total_percent: cpu_total as f32,
            per_core_mhz: per_core.iter().map(|u| 3_000 + (*u as u64) * 25).collect(),
            per_core_percent: per_core,
            load_average: Some([
                0.4 + 22.0 * build,
                3.0 + 9.0 * duty(t + 30.0, 120.0, 90.0),
                6.2,
            ]),
            temperatures: vec![
                temp(k10, 1, "Tctl", tctl),
                temp(k10, 3, "Tccd1", tctl - 8.0),
                temp(k10, 4, "Tccd2", tctl - 9.0),
            ],
        },
        memory: MemoryStats {
            total_bytes: 96 * GIB,
            used_bytes: (used_memory * GIB as f64) as u64,
            available_bytes: ((96.0 - used_memory) * GIB as f64) as u64,
            free_bytes: 12 * GIB,
            cached_bytes: Some(44 * GIB),
            swap_total_bytes: 8 * GIB,
            swap_used_bytes: 256 << 20,
        },
        gpus: vec![
            GpuStats {
                index: 0,
                vendor: "NVIDIA".to_owned(),
                name: "NVIDIA GeForce RTX 4090".to_owned(),
                utilization_percent: Some(gpu0_util.clamp(0.0, 100.0) as f32),
                memory_utilization_percent: Some((gpu0_util * 0.6) as f32),
                memory_used_bytes: Some(21 * GIB + GIB / 3),
                memory_total_bytes: Some(24 * GIB),
                temperature_c: Some((52.0 + 0.2 * gpu0_util) as f32),
                power_w: Some(gpu0_w as f32),
                power_limit_w: Some(450.0),
                fan_percent: Some((30.0 + 0.3 * gpu0_util) as f32),
                clock_mhz: Some(2_520),
                processes: vec![GpuProcess {
                    pid: train_pid,
                    memory_bytes: Some(21 * GIB),
                    sm_percent: Some(gpu0_util.clamp(0.0, 100.0) as f32),
                }],
                ..Default::default()
            },
            GpuStats {
                index: 1,
                vendor: "NVIDIA".to_owned(),
                name: "NVIDIA RTX 4000 Ada Generation".to_owned(),
                utilization_percent: Some(gpu1_util as f32),
                memory_utilization_percent: Some((gpu1_util * 0.4) as f32),
                memory_used_bytes: Some(9 * GIB + GIB * 4 / 5),
                memory_total_bytes: Some(20 * GIB),
                temperature_c: Some((44.0 + 0.2 * gpu1_util) as f32),
                power_w: Some(gpu1_w as f32),
                power_limit_w: Some(130.0),
                fan_percent: Some((30.0 + 0.4 * gpu1_util) as f32),
                clock_mhz: Some(if gpu1_util > 5.0 { 2_175 } else { 210 }),
                processes: vec![GpuProcess {
                    pid: ollama_pid,
                    memory_bytes: Some(9 * GIB + GIB / 2),
                    sm_percent: Some(gpu1_util as f32),
                }],
                ..Default::default()
            },
        ],
        sensors: vec![
            SensorChip {
                key: k10.to_owned(),
                name: "k10temp".to_owned(),
                temperatures: vec![
                    temp(k10, 1, "Tctl", tctl),
                    temp(k10, 3, "Tccd1", tctl - 8.0),
                    temp(k10, 4, "Tccd2", tctl - 9.0),
                ],
                fans: Vec::new(),
            },
            SensorChip {
                key: nct.to_owned(),
                name: "nct6686".to_owned(),
                temperatures: vec![
                    temp(nct, 1, "Thermistor 14", 38.0 + 0.1 * cpu_total),
                    temp(nct, 2, "AMD TSI Addr 98h", tctl - 2.0),
                ],
                fans: vec![
                    fan(nct, 1, 850.0 + 9.0 * cpu_total),
                    fan(nct, 2, 0.0),
                    fan(nct, 3, 780.0 + 4.0 * cpu_total),
                ],
            },
            nvme(0, drive_temp(0)),
            nvme(1, drive_temp(1)),
            nvme(2, drive_temp(2)),
        ],
        power: PowerStats {
            cpu_package_w: Some(cpu_w as f32),
            gpus_w: Some((gpu0_w + gpu1_w) as f32),
            igpu_w: None,
            total_w: Some((cpu_w + gpu0_w + gpu1_w) as f32),
            battery: None,
        },
        disks: (0..3).map(disk_io).collect(),
        filesystems: vec![
            FilesystemUsage {
                device: "/dev/nvme0n1p2".to_owned(),
                fs_type: "btrfs".to_owned(),
                mount_points: vec!["/".to_owned(), "/home".to_owned()],
                total_bytes: 5 * TIB + TIB / 2,
                available_bytes: 3 * TIB + TIB / 10,
            },
            FilesystemUsage {
                device: "/dev/nvme0n1p1".to_owned(),
                fs_type: "vfat".to_owned(),
                mount_points: vec!["/boot/efi".to_owned()],
                total_bytes: GIB,
                available_bytes: GIB - (60 << 20),
            },
        ],
        raid: RaidStats {
            btrfs: vec![BtrfsFilesystem {
                fsid: "6f1c2a9e-0d4b-4e58-9a3e-2b7c51d0e8f4".to_owned(),
                label: String::new(),
                mount_points: vec!["/".to_owned(), "/home".to_owned()],
                devices: vec![
                    "nvme0n1p2".to_owned(),
                    "nvme1n1".to_owned(),
                    "nvme2n1".to_owned(),
                ],
                device_stats: (1..=3)
                    .map(|devid| BtrfsDeviceStats {
                        devid,
                        name: if devid == 1 {
                            "nvme0n1p2".to_owned()
                        } else {
                            format!("nvme{}n1", devid - 1)
                        },
                        size_bytes: Some(2 * TIB - (1 << 33)),
                        used_bytes: Some(TIB - (1 << 36)),
                        ..Default::default()
                    })
                    .collect(),
                allocations: vec![
                    BtrfsAllocation {
                        kind: "data".to_owned(),
                        profile: "raid0".to_owned(),
                        total_bytes: 2 * TIB + TIB / 2,
                        used_bytes: 2 * TIB + TIB / 3,
                    },
                    BtrfsAllocation {
                        kind: "metadata".to_owned(),
                        profile: "raid1c3".to_owned(),
                        total_bytes: 12 * GIB,
                        used_bytes: 9 * GIB,
                    },
                    BtrfsAllocation {
                        kind: "system".to_owned(),
                        profile: "raid1c3".to_owned(),
                        total_bytes: 32 << 20,
                        used_bytes: 1 << 20,
                    },
                ],
                scrub: Some(BtrfsScrub {
                    started_at_ms: now_ms() - 3 * 86_400_000,
                    duration_secs: 1_920,
                    state: "finished".to_owned(),
                    bytes_scrubbed: 2 * TIB + TIB / 3,
                    ..Default::default()
                }),
            }],
            md: Vec::new(),
        },
        smart: (0..3).map(smart).collect(),
        networks: vec![
            NetworkStats {
                name: "enp9s0".to_owned(),
                rx_bytes_per_sec: 280e6 * download + 60e3 * wobble(12, t, 1.5),
                tx_bytes_per_sec: 4e6 * download + 25e3 * wobble(13, t, 2.0),
                link_speed_mbps: Some(10_000),
                link_up: Some(true),
                ..Default::default()
            },
            NetworkStats {
                name: "tailscale0".to_owned(),
                rx_bytes_per_sec: 9e3 * wobble(14, t, 3.0),
                tx_bytes_per_sec: 40e3 * wobble(15, t, 4.0),
                rx_total_bytes: 3 * GIB,
                tx_total_bytes: 9 * GIB,
                link_up: Some(true),
                ..Default::default()
            },
        ],
        processes,
        ..Default::default()
    }
}

fn laptop(t: f64) -> Snapshot {
    let busy = (wobble(40, t, 8.0) - 0.3).max(0.0) / 0.7;
    let cpu_total = 6.0 + 35.0 * busy;
    let per_core = cores(200, t, 16, cpu_total);
    let tctl = 46.0 + 0.5 * cpu_total;
    let soc_w = 5.0 + 0.4 * cpu_total;
    let k10 = "k10temp:0000:00:18.3";
    Snapshot {
        host: HostInfo {
            hostname: "laptop".to_owned(),
            os: "Linux (CachyOS)".to_owned(),
            kernel: "7.0.2-cachyos".to_owned(),
            uptime_secs: 8 * 86_400 + 10 * 3600 + t as u64,
            // An agent nobody updated yet.
            rigtop_version: "0.8.2".to_owned(),
        },
        cpu: CpuStats {
            model: "AMD Ryzen 7 PRO 7840U w/ Radeon 780M Graphics".to_owned(),
            total_percent: cpu_total as f32,
            per_core_mhz: per_core.iter().map(|u| 1_400 + (*u as u64) * 35).collect(),
            per_core_percent: per_core,
            load_average: Some([0.8 + 3.0 * busy, 1.6, 1.4]),
            temperatures: vec![temp(k10, 1, "Tctl", tctl)],
        },
        memory: MemoryStats {
            total_bytes: 30 * GIB,
            used_bytes: 15 * GIB + (busy * 2.0 * GIB as f64) as u64,
            available_bytes: 14 * GIB,
            free_bytes: 3 * GIB,
            cached_bytes: Some(10 * GIB),
            swap_total_bytes: 30 * GIB,
            swap_used_bytes: 0,
        },
        gpus: vec![GpuStats {
            index: 0,
            vendor: "AMD".to_owned(),
            name: "AMD Radeon 780M".to_owned(),
            integrated: true,
            utilization_percent: Some((4.0 + 20.0 * busy) as f32),
            memory_used_bytes: Some(GIB + GIB / 2),
            memory_total_bytes: Some(2 * GIB),
            temperature_c: Some((tctl - 3.0) as f32),
            clock_mhz: Some(800),
            ..Default::default()
        }],
        sensors: vec![SensorChip {
            key: k10.to_owned(),
            name: "k10temp".to_owned(),
            temperatures: vec![temp(k10, 1, "Tctl", tctl)],
            fans: Vec::new(),
        }],
        power: PowerStats {
            igpu_w: Some(soc_w as f32),
            total_w: Some(soc_w as f32),
            battery: Some(BatteryStats {
                percent: Some((78.0 - t / 600.0).max(5.0) as f32),
                status: "Discharging".to_owned(),
                power_w: Some((soc_w + 3.0) as f32),
            }),
            ..Default::default()
        },
        disks: vec![DiskIo {
            name: "nvme0n1".to_owned(),
            read_bytes_per_sec: 6e6 * busy,
            write_bytes_per_sec: 1e6 * busy,
            busy_percent: Some((4.0 * busy) as f32),
            temperature_c: Some((38.0 + 4.0 * busy) as f32),
        }],
        filesystems: vec![FilesystemUsage {
            device: "/dev/nvme0n1p2".to_owned(),
            fs_type: "ext4".to_owned(),
            mount_points: vec!["/".to_owned()],
            total_bytes: 930 * GIB,
            available_bytes: 352 * GIB,
        }],
        networks: vec![NetworkStats {
            name: "wlp1s0".to_owned(),
            rx_bytes_per_sec: 1.5e6 * busy + 20e3 * wobble(41, t, 2.0),
            tx_bytes_per_sec: 0.3e6 * busy + 8e3 * wobble(42, t, 3.0),
            link_up: Some(true),
            wireless: Some(WirelessLink {
                rx_bitrate_mbps: Some(1_201.0),
                tx_bitrate_mbps: Some(864.0),
                signal_dbm: Some(-54),
            }),
            ..Default::default()
        }],
        processes: vec![
            process(
                2_810,
                "plasmashell",
                "/usr/bin/plasmashell",
                "demo",
                1.0 + 8.0 * busy,
                520 << 20,
                24,
                8 * 86_400,
            ),
            process(
                4_420,
                "code",
                "/opt/visual-studio-code/code",
                "demo",
                2.0 + 30.0 * busy,
                900 << 20,
                38,
                86_400,
            ),
            process(
                4_980,
                "firefox",
                "/usr/lib/firefox/firefox",
                "demo",
                1.5 + 10.0 * wobble(43, t, 6.0),
                1_800 << 20,
                96,
                3 * 86_400,
            ),
            process(
                5_102,
                "rigtop",
                "rigtop --agent",
                "demo",
                0.6,
                10 << 20,
                3,
                3_600,
            ),
            process(
                1,
                "systemd",
                "/sbin/init",
                "root",
                0.0,
                12 << 20,
                1,
                8 * 86_400,
            ),
        ],
        ..Default::default()
    }
}

fn nas(t: f64) -> Snapshot {
    let cpu_total = 18.0 + 10.0 * wobble(60, t, 5.0);
    let per_core = cores(300, t, 4, cpu_total);
    let resync = 42.0 + t / 120.0;
    let disks = ["sda", "sdb", "sdc", "sdd"];
    let smart = |i: usize| SmartDevice {
        device: disks[i].to_owned(),
        protocol: "ATA".to_owned(),
        model: "WDC WD80EFPX-68C4ZN0".to_owned(),
        passed: Some(true),
        temperature_c: Some(38.0 + i as f32),
        power_on_hours: Some(21_400 + 200 * i as u64),
        reallocated_sectors: Some(if i == 2 { 8 } else { 0 }),
        pending_sectors: Some(0),
        offline_uncorrectable: Some(0),
        collected_at_ms: Some(now_ms() - 300_000),
        ..Default::default()
    };
    Snapshot {
        host: HostInfo {
            hostname: "nas".to_owned(),
            os: "Linux (Debian 13)".to_owned(),
            kernel: "6.12.48-amd64".to_owned(),
            uptime_secs: 41 * 86_400 + t as u64,
            rigtop_version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        cpu: CpuStats {
            model: "Intel(R) Celeron(R) N5105 @ 2.00GHz".to_owned(),
            total_percent: cpu_total as f32,
            per_core_mhz: vec![2_000; 4],
            per_core_percent: per_core,
            load_average: Some([1.9, 2.1, 2.0]),
            temperatures: vec![temp("coretemp:coretemp.0", 1, "Package id 0", 52.0)],
        },
        memory: MemoryStats {
            total_bytes: 16 * GIB,
            used_bytes: 3 * GIB,
            available_bytes: 12 * GIB,
            free_bytes: GIB,
            cached_bytes: Some(11 * GIB),
            ..Default::default()
        },
        disks: (0..4)
            .map(|i| DiskIo {
                name: disks[i].to_owned(),
                read_bytes_per_sec: 150e6 + 20e6 * wobble(61 + i as u64, t, 4.0),
                write_bytes_per_sec: if i == 3 { 148e6 } else { 1e6 },
                busy_percent: Some(88.0 + 8.0 * wobble(65 + i as u64, t, 3.0) as f32),
                temperature_c: Some(38.0 + i as f32),
            })
            .collect(),
        filesystems: vec![
            FilesystemUsage {
                device: "/dev/md0".to_owned(),
                fs_type: "ext4".to_owned(),
                mount_points: vec!["/srv".to_owned()],
                total_bytes: 21 * TIB + TIB / 2,
                available_bytes: 2 * TIB - TIB / 5,
            },
            FilesystemUsage {
                device: "/dev/sde2".to_owned(),
                fs_type: "ext4".to_owned(),
                mount_points: vec!["/".to_owned()],
                total_bytes: 230 * GIB,
                available_bytes: 180 * GIB,
            },
        ],
        raid: RaidStats {
            btrfs: Vec::new(),
            md: vec![MdArray {
                name: "md0".to_owned(),
                active: true,
                level: "raid5".to_owned(),
                members: disks
                    .iter()
                    .enumerate()
                    .map(|(role, name)| MdMember {
                        name: (*name).to_owned(),
                        role: role as u32,
                        ..Default::default()
                    })
                    .collect(),
                expected_devices: Some(4),
                working_devices: Some(4),
                sync: Some(MdSync {
                    action: "resync".to_owned(),
                    percent: resync.min(99.9) as f32,
                    finish_minutes: Some((100.0 - resync).max(0.0) as f32 * 9.0),
                    speed_kib_per_sec: Some(148_000),
                }),
                ..Default::default()
            }],
        },
        smart: (0..4).map(smart).collect(),
        networks: vec![NetworkStats {
            name: "eno1".to_owned(),
            rx_bytes_per_sec: 2e6 * wobble(70, t, 6.0),
            tx_bytes_per_sec: 95e6 * ((wobble(71, t, 10.0) - 0.3).max(0.0) / 0.7),
            link_speed_mbps: Some(2_500),
            link_up: Some(true),
            ..Default::default()
        }],
        processes: vec![
            // The kernel's resync thread is not counted per process.
            process(812, "md0_resync", "", "root", 9.0, 0, 1, 3 * 3600),
            // Serves the files the network panel sends.
            with_io(
                process(
                    1_204,
                    "smbd",
                    "/usr/sbin/smbd --foreground --no-process-group",
                    "root",
                    6.0 + 20.0 * wobble(72, t, 10.0),
                    120 << 20,
                    4,
                    41 * 86_400,
                ),
                95e6 * ((wobble(71, t, 10.0) - 0.3).max(0.0) / 0.7),
                1e6 * wobble(74, t, 5.0),
            ),
            with_io(
                process(
                    1_330,
                    "jellyfin",
                    "/usr/lib/jellyfin/bin/jellyfin",
                    "jellyfin",
                    3.0 + 4.0 * wobble(73, t, 7.0),
                    700 << 20,
                    30,
                    41 * 86_400,
                ),
                4e6 + 4e6 * wobble(75, t, 7.0),
                0.0,
            ),
            process(
                1_002,
                "rigtop",
                "rigtop --agent",
                "admin",
                0.4,
                9 << 20,
                3,
                3_600,
            ),
            process(
                1,
                "systemd",
                "/sbin/init",
                "root",
                0.0,
                11 << 20,
                1,
                41 * 86_400,
            ),
        ],
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{Severity, alerts};

    #[test]
    fn demo_hosts_change_over_time_and_repeat_exactly() {
        let mut a = DemoSource::new(DemoHost::Workstation);
        let mut b = DemoSource::new(DemoHost::Workstation);
        let first = a.poll().unwrap();
        b.poll().unwrap();
        let mut later = first.clone();
        for _ in 0..30 {
            later = a.poll().unwrap();
            b.poll().unwrap();
        }
        assert_ne!(first.cpu.total_percent, later.cpu.total_percent);
        assert_eq!(
            b.poll().unwrap().cpu.total_percent,
            a.poll().unwrap().cpu.total_percent
        );
        // Traffic adds up.
        assert!(later.networks[0].rx_total_bytes > first.networks[0].rx_total_bytes);
        assert_eq!(later.process_count, later.processes.len());
        // GPU processes are in the process list.
        for gpu in &later.gpus {
            for p in &gpu.processes {
                assert!(later.processes.iter().any(|q| q.pid == p.pid));
            }
        }
    }

    #[test]
    fn the_nas_needs_attention() {
        let snapshot = DemoSource::new(DemoHost::Nas).poll().unwrap();
        let found = alerts(&snapshot);
        let texts: Vec<&str> = found.iter().map(|a| a.text.as_str()).collect();
        assert_eq!(found[0].severity, Severity::Critical);
        assert!(texts.contains(&"sdc: SMART warning"), "{texts:?}");
        assert!(
            texts.iter().any(|t| t.starts_with("md0 resync")),
            "{texts:?}"
        );
        assert!(texts.iter().any(|t| t.starts_with("/srv ")), "{texts:?}");
        assert!(alerts(&DemoSource::new(DemoHost::Workstation).poll().unwrap()).is_empty());
    }

    #[test]
    fn wobble_stays_in_range() {
        for i in 0..1000 {
            let v = wobble(7, i as f64 * 0.37, 3.0);
            assert!((0.0..=1.0).contains(&v));
        }
    }
}
