# Changelog

All notable changes to rigtop are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/) (before 1.0, minor versions may
include breaking changes).

## [Unreleased]

## [0.16.0] - 2026-10-29

### Added
- ZFS pools (Linux): each imported pool's health from
  `/proc/spl/kstat/zfs/<pool>/state` (no root needed) in the storage box
  and on overview cards. `DEGRADED`, `FAULTED`, `UNAVAIL`, `REMOVED` and
  `SUSPENDED` raise a critical alert, other states than `ONLINE` a warning.
- Failed systemd units (Linux): `systemctl list-units --state=failed`,
  looked up every 30 seconds, raises a warning per unit
  (`systemd: nginx.service failed`); `[alerts] ignored_units` lists units
  not to warn about.
- The alert history (TUI `a`, GUI "alerts…") is kept between runs, in
  `~/.local/state/rigtop/alerts-tui.jsonl` / `alerts-gui.jsonl` (the local
  data directory on macOS and Windows), so what happened overnight is
  still there after a restart. Alerts still present when rigtop starts
  are not logged again. Older events show their date.
- Homebrew: `brew install akivajp/tap/rigtop` (macOS on Apple Silicon,
  Linux x86_64 and aarch64), from the release binaries. rigtop's own
  update leaves a Homebrew install alone and points to `brew upgrade rigtop`.

## [0.15.0] - 2026-10-29

### Added
- Remote hosts that cannot be reached say for how long and since when, in
  their overview card, tab (GUI: on hover) and title: `offline for 5m
  (since 14:03)`, counted from their last data.
- Claude Code widget: a `ctx` row under each session with how full its
  context window is (yellow from 70%, red from 90%), from the last reply in
  its transcript; 1M tokens for current Claude models and `[1m]` ones,
  200K for older ones (`RIGTOP_CLAUDE_CONTEXT_WINDOW` overrides it).

## [0.14.0] - 2026-10-29

### Changed
- Remote hosts: when `rigtop` is not on the PATH of the SSH session, the
  agent is looked for in `~/.local/bin` (where the install script puts it)
  before `~/.cargo/bin`, both for monitoring and for updating it.

## [0.13.0] - 2026-10-28

### Added
- Update checks: rigtop looks for a new release on github.com at startup
  and every hour, and says so in the title. `U` in the TUI, or the button
  in the GUI, installs it in place of the running program after you
  confirm (through the release's `install.sh`); it runs after a restart,
  which the GUI offers. The GUI also announces a new release with a desktop
  notification (Linux and macOS). Copies from `cargo install` or a package
  manager get the command to update them, and Windows gets directions.
  `[updates]` sets `check`, `interval` (e.g. `"1h"`, `"30m"`, `"1d"`) and
  `notify`.

## [0.12.0] - 2026-10-28

### Added
- Edits to the configuration file apply while rigtop runs (TUI and GUI),
  checked every second: sensor names, alert thresholds, notifications and
  the overview at once; hosts added to `[[hosts]]` connect and removed ones
  disconnect, while the others keep their connection and history; widgets,
  and this machine's collector for `[smart]`, `[btrfs]` and `[lhm]`,
  restart. A file that cannot be read is reported and the settings in use
  are kept.
- Updating rigtop on remote hosts that run an older version: `u` in the
  TUI, or a click on the "agent 0.10.1 · update…" note in the GUI. After
  you confirm, the release's `install.sh` of your version runs over SSH and
  replaces the rigtop the agent runs, then the host reconnects. Copies from
  `cargo install` or a package manager, and Windows hosts, get directions
  instead.

## [0.11.0] - 2026-10-28

### Added
- Process table: a Disk column with each process's reads plus writes per
  second, sortable (in the GUI, hovering shows reads and writes apart).
  Blank when idle, and for other users' processes on Linux, whose counters
  only root can read; on Windows the counters include all of a process's
  I/O, not only disks. Narrow tables give up Thr before it.
- TUI: alert history (`a`), like the GUI's: the alerts of every host as
  they appeared and cleared, with times. Until opened, the tab bar (or the
  CPU box title with one host) shows the number of new alerts in the color
  of the worst.
- Process tree: `e` in the TUI, "tree" in the GUI. Processes go under
  their parents, siblings in the sort order; a filter keeps the parents of
  the processes it finds.

### Fixed
- macOS: the memory panel's Avail no longer overlaps Used (their sum was
  more than the total). Avail is now what Used leaves of the total, as in
  Activity Monitor, and Cached shows the file-backed pages (#1).

## [0.10.1] - 2026-10-28

### Added
- An install script for Linux and macOS, attached to each release:
  `curl -fsSL https://github.com/akivajp/rigtop/releases/latest/download/install.sh | sh`
  puts the binaries in `~/.local/bin` after checking them against
  `SHA256SUMS` (`--version`, `--bin-dir`, `--tui-only`).

### Fixed
- macOS: no more "NVIDIA GPUs unavailable" note about `nvml.dll`; NVML,
  which NVIDIA does not ship for macOS, is no longer loaded there (#2).

## [0.10.0] - 2026-10-27

### Added
- Graph time spans of 6 and 24 hours, drawn from per-minute averages kept
  beside the samples (a day at the default interval, about 12 KB per graph).
- GUI: alert history. "alerts…" (in the tab bar, or the CPU panel title with
  one host) lists the alerts of every host as they appeared and cleared,
  with times; until opened it shows the number of new alerts in the color of
  the worst. Useful where desktop notifications are off (Windows).
- An application icon: in the Windows executable (Explorer, Start menu),
  the GUI window, and the Debian package's desktop entry
  (`crates/rigtop-gui/assets/rigtop.svg`).
- Remote hosts running another rigtop version show it in their title
  (`agent 0.8.2`, with a hint to update) in the TUI and the GUI. Snapshots
  carry the version of the rigtop that took them; for older agents it comes
  from their greeting.

## [0.9.0] - 2026-10-26

### Added
- Graph time span: fit to the width (one sample per pixel or braille dot,
  as before), or the last 1, 5, 15 minutes or hour, averaged or stretched to
  the width; `g` in the TUI, the "graph" button in the GUI's CPU panel title
  (remembered between runs). The same span applies to every host. Graphs
  keep 4096 samples (over an hour at the default interval).
- `[alerts]` in the configuration sets when readings become alerts:
  filesystem usage (90% warning, 97% critical), and new warnings for hot
  CPUs (96 °C), GPUs (85 °C) and drives (70 °C, from SMART or the NVMe
  sensor); 0 turns one off. Remote hosts are judged by the viewer's
  settings.
- GUI: a "Sensors" window (the "sensors…" button under the CPU cores, or a
  click on a sensor's name) lists every temperature and fan channel with its
  reading, to name it or hide it; saved to `[sensors]` in the configuration
  file with the rest of the file left as it was.
- Debian packages in the releases (`amd64` with the GUI and a desktop entry,
  `arm64` with the terminal UI), and a Scoop manifest for Windows
  (`packaging/scoop/rigtop.json`, adds a Start menu shortcut).
- `--demo` (TUI and GUI): made-up data for three hosts (a workstation
  training on two GPUs, a laptop on battery, a NAS with alerts) to try the
  interface; every poll is a second of made-up time, so a short
  `--interval-ms` fast-forwards. The README screenshots use it.

### Fixed
- TUI: the CPU model no longer runs into the core grid next to the graph

## [0.8.2] - 2026-10-26

### Added

- GUI: a "widgets" button (next to "hosts…") hides and shows the widget
  column, remembered between runs

### Fixed

- GUI on narrow windows (e.g. with the widget column): the process table
  drops Command, Thr, User, VRAM and Gpu% in that order instead of hiding
  Cpu% and Mem off the edge; the CPU model no longer runs into the core
  grid, which also gets a denser cell for short panels; the CPU info rows,
  GPU names, drive rows and btrfs rows wrap instead of being cut off; link
  speeds move to a tooltip so network rates stay visible; the title clock
  hides when the controls need its place; the overview puts cards side by
  side when one column would leave them too short for all their sections

## [0.8.1] - 2026-10-26

### Added

- Widgets: `"gauge_style": "level"` draws a row's gauge in its level color,
  for amounts left (where a full bar is good)
- `contrib/widgets/claude-code/`: usage windows shown as what is left
  (`RIGTOP_CLAUDE_QUOTA=used` for usage); idle sessions show for how long
  and whether they wait for you or for background tasks and subagents;
  session names without the project prefix

## [0.8.0] - 2026-10-26

### Added

- Widgets: a column on the right (GUI and TUI, in the overview and host
  views) with a built-in large clock and `[[widgets]]` fed by any command
  that prints text or JSON rows (labels, values, gauges, colors); `w`
  toggles it in the TUI
- `contrib/widgets/claude-code/`: a widget showing Claude Code's usage
  limits (5-hour and weekly), running sessions and active subagents

## [0.7.1] - 2026-10-26

### Fixed

- Windows: NVMe drive temperatures from LibreHardwareMonitor ("Composite
  Temperature", or numbered sensors), and values it sends as text with
  units

## [0.7.0] - 2026-10-26

### Added

- GUI (Linux, macOS): desktop notifications when a new alert appears on
  any host, also while minimized (`[notifications] level`: `critical` by default,
  `warning` or `off`); alerts present at startup are not announced;
  `rigtop-gui --test-notification` shows a sample one
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

[Unreleased]: https://github.com/akivajp/rigtop/compare/v0.16.0...HEAD
[0.16.0]: https://github.com/akivajp/rigtop/compare/v0.15.0...v0.16.0
[0.15.0]: https://github.com/akivajp/rigtop/compare/v0.14.0...v0.15.0
[0.14.0]: https://github.com/akivajp/rigtop/compare/v0.13.0...v0.14.0
[0.13.0]: https://github.com/akivajp/rigtop/compare/v0.12.0...v0.13.0
[0.12.0]: https://github.com/akivajp/rigtop/compare/v0.11.0...v0.12.0
[0.11.0]: https://github.com/akivajp/rigtop/compare/v0.10.1...v0.11.0
[0.10.1]: https://github.com/akivajp/rigtop/compare/v0.10.0...v0.10.1
[0.10.0]: https://github.com/akivajp/rigtop/compare/v0.9.0...v0.10.0
[0.9.0]: https://github.com/akivajp/rigtop/compare/v0.8.2...v0.9.0
[0.8.2]: https://github.com/akivajp/rigtop/compare/v0.8.1...v0.8.2
[0.8.1]: https://github.com/akivajp/rigtop/compare/v0.8.0...v0.8.1
[0.8.0]: https://github.com/akivajp/rigtop/compare/v0.7.1...v0.8.0
[0.7.1]: https://github.com/akivajp/rigtop/compare/v0.7.0...v0.7.1
[0.7.0]: https://github.com/akivajp/rigtop/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/akivajp/rigtop/compare/v0.5.1...v0.6.0
[0.5.1]: https://github.com/akivajp/rigtop/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/akivajp/rigtop/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/akivajp/rigtop/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/akivajp/rigtop/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/akivajp/rigtop/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/akivajp/rigtop/releases/tag/v0.1.0
