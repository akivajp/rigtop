# Changelog

All notable changes to rigtop are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/) (before 1.0, minor versions may
include breaking changes).

## [Unreleased]

## [0.21.0] - 2026-10-31

### Added
- "Stall" next to the load average (Linux): the share of time tasks waited
  for the CPU, memory or I/O (pressure stall information, the last 10
  seconds), for each one of at least 1%; the GUI's tooltip has the details.
  It explains slowness the usage figures miss, such as memory reclaim or a
  disk holding everything up. Waiting for memory over
  `[alerts] memory_pressure_warning_percent` (10%) of the last minute, and
  every task waiting for I/O over `io_pressure_warning_percent` (30%), are
  warnings.
- `[watch] processes = ["train.py", "rsync"]`: `rigtop --watch` finds
  these programs on every host, by name or by a file in their command line,
  and runs the command when one exits ("train.py (pid 1234) exited after
  3h12m"), whatever the `level`. Remote hosts need rigtop 0.21 or later.
- Pause and resume processes (SIGSTOP / SIGCONT), and change their
  priority by 5 nice steps: `p`, `]` and `[` in the TUI, the right-click
  menu in the GUI (Linux, macOS). Pausing asks first; paused processes are
  marked `stopped`, and the nice value is in the details. Remote hosts need
  rigtop 0.21 or later for these.
- Battery time left (Linux): until empty or full at the recent, smoothed
  rate (`72% -12W 2h33m`), and its health, the full charge as a share of
  the design capacity: in the tooltip, and next to the charge once below
  80% (`health 77%`).
- Disk request time in the storage panel (Linux): the average time a
  request took since the last sample (`0.3ms`, `12ms`), yellow from 100 ms
  and red from 500 ms, so a member of a RAID that slowed down (retrying
  reads, failing) stands out from the others.
- Alert thresholds per host: `[hosts.alerts]` under a `[[hosts]]` entry
  replaces the keys of `[alerts]` it gives for that host (a laptop that
  runs hot, a file server kept fuller). Typos fail the load, naming the
  host. The GUI's host editor keeps them.

### Changed
- Intel CPUs (Linux): each core's temperature is shown next to its meter,
  like btop, instead of a `Core 0`, `Core 1`, ... list after the load
  average; only `Package id 0` stays there. Remote hosts need rigtop 0.21
  or later for this; older ones show the list as before.

## [0.20.0] - 2026-10-30

### Added
- Widget rows can color their label (`label_level`), and headings take the
  row's `level`. A `"separator": true` row draws a line between sections.

### Changed
- Claude Code widget: working sessions come first, in green down to the
  context bar; sessions waiting for you follow, most recent first. A
  permission prompt stays yellow; a session whose turn ended is yellow for
  an hour, then grey on one line, and left out (counted on one line) after
  2 hours. Both times can be changed (`RIGTOP_CLAUDE_FRESH_SECS`,
  `RIGTOP_CLAUDE_HIDE_SECS`). A project's name is yellow while one of its
  sessions waits for you, else green while one works, else grey. A line
  separates the usage limits from the sessions.

## [0.19.0] - 2026-10-30

### Added
- Memory alerts: memory in use above `[alerts] memory_warning_percent`
  (90%, page cache not counted) is a warning, and a process stopped by the
  kernel's OOM killer is a critical alert for an hour (Linux; counted from
  `/proc/vmstat` while rigtop runs, with the likely process: the largest
  one that went away at that moment). GPU memory has
  `gpu_memory_warning_percent`, off by default since PyTorch, JAX and
  TensorFlow reserve most of the VRAM up front.
- "reboot needed" next to the uptime (Linux) when an update waits for a
  reboot: `/run/reboot-required` on Debian and Ubuntu (the packages in the
  tooltip), or a running kernel whose modules are gone (Arch and others).
- Notify when a process exits, e.g. a training run: `x` in the TUI (alert
  history, status line and terminal bell), "Notify when it exits" in the
  GUI's process menu (desktop notification, whatever the notification
  level, and alert history). Also works for hosts in the overview: the
  agent keeps watched processes in its limited process list
  (`watch_processes` in the agent protocol, announced in its greeting;
  older agents are left alone and the details say they cannot tell).
- Filesystems filling up: one that will be full within
  `[alerts] filesystem_full_warning_hours` (24) at the growth of the last
  half hour is a warning ("/srv full in ~5h12m"). Measured by the agent,
  so remote hosts need 0.19 or later; it takes ten minutes of history.

## [0.18.0] - 2026-10-29

### Added
- `rigtop --watch`: watches this machine and the configured hosts without
  a screen and runs the `[watch] command` when an alert appears or clears
  (`RIGTOP_HOST`, `RIGTOP_SEVERITY`, `RIGTOP_CHANGE`, `RIGTOP_ALERT` and
  `RIGTOP_MESSAGE` in its environment), e.g. to post to ntfy, a chat webhook
  or mail. A host unreachable for `offline_after` (3 minutes) is a critical
  alert. Its own alert history keeps a restart from sending alerts again.
  Meant to run as a systemd user service (see the README);
  `rigtop --test-alert` runs the command once with a sample alert.
- Linux: a Container column in the process table, for processes in Docker,
  Podman, Kubernetes (containerd, CRI-O), LXC or systemd-nspawn containers,
  from their cgroup. Names come from `docker ps` / `podman ps` when the user
  may run them (otherwise the short id); the column is hidden on hosts
  without containers. It can be sorted, and the filter matches it.
- NVIDIA: the GPU title shows why its clocks are held down ("power cap"
  when it runs at its power limit; "slowed: thermal", "hw thermal", "power
  brake" or "hw slowdown" when the hardware slows it down, which is also a
  warning) and ECC memory errors since the driver loaded (uncorrected ones
  are a critical alert). The GUI shows the PCIe link in the GPU name's
  tooltip.
- Process details: `Enter` in the TUI, a double-click (or right-click →
  "Details…") in the GUI shows the selected process's parents, children,
  user, container, start time, usage, GPU use and whole command line.

### Changed
- The overview puts hosts that need attention first (unreachable or with a
  critical alert, then with warnings); the others keep their order. Tabs
  keep the configured order. `[overview] alerts_first = false` turns this
  off.

## [0.17.0] - 2026-10-29

### Added
- Panels can be hidden: "panels…" in the GUI (next to "reset layout";
  kept with the layout, which "reset layout" restores) and `v` in the TUI,
  then `m`, `g`, `s`, `n`, `p` for memory, GPU, storage, network and
  processes (kept in the configuration as `[tui] hidden_panels`). The
  other panels take the room; the CPU panel always stays.
- The widget column can also be switched from those menus ("Widgets" in
  "panels…", `w` in the TUI's `v` menu), next to the existing "widgets"
  button and `w` key. The TUI now keeps the choice as
  `[tui] show_widgets`, so it survives a restart.

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

[Unreleased]: https://github.com/akivajp/rigtop/compare/v0.21.0...HEAD
[0.21.0]: https://github.com/akivajp/rigtop/compare/v0.20.0...v0.21.0
[0.20.0]: https://github.com/akivajp/rigtop/compare/v0.19.0...v0.20.0
[0.19.0]: https://github.com/akivajp/rigtop/compare/v0.18.0...v0.19.0
[0.18.0]: https://github.com/akivajp/rigtop/compare/v0.17.0...v0.18.0
[0.17.0]: https://github.com/akivajp/rigtop/compare/v0.16.0...v0.17.0
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
