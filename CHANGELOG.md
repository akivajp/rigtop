# Changelog

All notable changes to rigtop are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/) (before 1.0, minor versions may
include breaking changes).

## [Unreleased]

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

[Unreleased]: https://github.com/akivajp/rigtop/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/akivajp/rigtop/releases/tag/v0.1.0
