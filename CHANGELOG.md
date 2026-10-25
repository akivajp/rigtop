# Changelog

All notable changes to rigtop are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/) (before 1.0, minor versions may
include breaking changes).

## [Unreleased]

### Added

- GUI: desktop notifications when a new alert appears on any host, also
  while minimized (`[notifications] level`: `critical` by default,
  `warning` or `off`); alerts present at startup are not announced
- Windows: drive health from LibreHardwareMonitor (wear, spare capacity,
  temperature, power-on hours) in the SMART rows, as `disk0`, `disk1`...
  like Disk Management; drives without a disk row also show their
  temperature (and the model on hover in the GUI)

### Changed

- NVIDIA: per-process GPU utilization, the costliest NVML query (about
  2 ms per GPU), is skipped for GPUs no process uses and while only the
  overview's top processes are wanted

## [0.6.0] - 2026-10-26

### Added

- Linux: AMD and Intel GPUs in the GPU panel and on overview cards (usage,
  VRAM, temperature, clock, and power of discrete cards) from sysfs, named
  from the driver or the PCI ID database; unknown values show as `--`; the
  GUI's GPU panel switches to shorter rows when several GPUs do not fit
- `sudo rigtop --install-collector` sets up the root timer that collects
  SMART data and btrfs scrub results (the files of `contrib/collect/` are
  built in, so `cargo install` is enough); `--uninstall-collector` removes it

### Changed

- btrfs: errors are reported per device name (`nvme0n1: 3`) instead of
  btrfs device id, looked up with the `BTRFS_IOC_DEV_INFO` ioctl (no root
  needed); alerts name the affected drives too

## [0.5.1] - 2026-10-26

### Fixed

- Overview: Linux hosts listed fewer processes than configured, because
  busy kernel threads took places in the top list the agent sends and
  were then hidden; the agent now leaves them out
- Overview: the process list (and the GUI's swap row) keeps its height
  when there are fewer entries, so the sections below no longer jump

## [0.5.0] - 2026-10-25

### Added

- Laptops (Linux): battery charge and charge/discharge power in the Power
  line (`Bat 72% -12W`) and on overview cards; batteries of mice and
  keyboards are left out
- AMD APUs (Linux): without readable RAPL, the APU's SoC power (amdgpu
  `PPT`) is shown as `SoC` and counted in the total
- README: a Linux section with the packages per distribution (Debian /
  Ubuntu and Arch) and a udev rule that lets the admin group read RAPL

### Changed

- Discrete AMD GPUs' power is no longer reported as integrated GPU power
- Linux: collecting processes no longer refreshes every thread, which cut
  the agent's CPU use to about a quarter on a desktop with thousands of
  browser threads; thread counts now come from `/proc/<pid>/stat`
- Remote hosts: when `rigtop` is not on the `PATH` of the SSH session,
  the default agent command falls back to `~/.cargo/bin/rigtop --agent`

## [0.4.0] - 2026-10-25

### Added

- Windows: temperatures, fans and CPU package power from
  LibreHardwareMonitor's web server (`data.json`, default
  `http://127.0.0.1:8085`; `[lhm] url` in the configuration), with a
  note in the CPU panel while it cannot be read
- Windows GUI: "set up…" installs LibreHardwareMonitor with winget when
  needed, turns its web server on for this computer only and starts it

## [0.3.0] - 2026-10-25

### Added

- Multi-host monitoring: hosts from `[[hosts]]` in the configuration and
  repeated `--host` options, each with its own tab (this machine
  included), plus an overview of all hosts that rigtop opens on
- Overview cards list alerts (storage errors, missing devices, failing
  SMART, degraded or resyncing md arrays, nearly full filesystems, hot
  GPUs) and then sections in priority order (CPU, GPU, memory, top
  processes, network, storage by default) as far as there is room; the
  order is configurable globally (`[overview] sections`), per host
  (`sections`) and in the GUI per card; `[overview] include_local` and a
  toggle decide whether this machine gets a card
- TUI: tab bar with status dots, `Tab` / `0`-`9` / mouse to switch views,
  `l` to toggle this machine in the overview
- Hosts not shown in detail send only their top processes
  (`[overview] processes`, default 10)
- GUI: "hosts…" window to add, edit, test and remove remote hosts; they
  are saved to `[[hosts]]` in the configuration file, leaving the rest of
  the file untouched

### Changed

- `--host` can be given several times, and `rigtop --host x` now opens the
  overview of this machine and `x` instead of `x` alone

## [0.2.0] - 2026-10-25

### Added

- Remote monitoring over SSH: `rigtop --host <destination>` and
  `rigtop-gui --host <destination>` run `rigtop --agent` on the remote host
  through the system `ssh` and show its data, including killing processes
  there. No daemon or open port is needed; the agent exits with the
  connection and the client reconnects automatically. `--remote-command`
  sets the remote command
- `rigtop --agent`: streams snapshots as JSON lines on stdout and takes
  commands on stdin (protocol version 1)

### Fixed

- `rigtop-gui` no longer grows its memory while the window is hidden
  (minimized or screen locked): data is now taken in even when nothing is
  drawn, and the queue between the collector and the UI is bounded

## [0.1.0] - 2026-10-25

First release.

### Added

- `rigtop`: btop-inspired terminal UI
  - CPU total graph and per-core meters, CPU temperatures (Tctl/Tccd on AMD),
    CPU package + GPU power total, fan speeds
  - Memory, GPUs (NVIDIA via NVML: utilization, VRAM, temperature, power),
    network with link speeds (Wi-Fi bit rate and signal on Linux)
  - Storage health: filesystem usage, per-device I/O with NVMe temperature,
    SMART status and age, btrfs RAID profiles with live error counters and
    the last scrub, mdadm array state
  - Process list with per-process GPU usage and VRAM, sorting, filtering,
    kernel thread toggle, SIGTERM/SIGKILL behind a confirmation, mouse support
  - Layout adapts to the terminal size; clock in the CPU box
  - `--dump-json` prints one snapshot for scripting and debugging
- `rigtop-gui`: native GUI (egui) with the same panels, resizable and
  rearrangeable, zoomable (Ctrl + wheel, Ctrl + `+`/`-`/`0`), with the layout
  and zoom restored on start
- Configuration file (`config.toml`) for sensor names and hidden channels
- `contrib/collect`: root systemd timer that collects SMART data and btrfs
  scrub results for rigtop without running rigtop as root
- Windows support (CPU, memory, processes, network with link speeds, drives,
  NVIDIA GPUs; temperatures, fans and CPU power are not available yet)

[Unreleased]: https://github.com/akivajp/rigtop/compare/v0.6.0...HEAD
[0.6.0]: https://github.com/akivajp/rigtop/compare/v0.5.1...v0.6.0
[0.5.1]: https://github.com/akivajp/rigtop/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/akivajp/rigtop/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/akivajp/rigtop/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/akivajp/rigtop/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/akivajp/rigtop/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/akivajp/rigtop/releases/tag/v0.1.0
