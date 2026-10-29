# rigtop

<img src="crates/rigtop-gui/assets/rigtop.svg" alt="rigtop icon" width="96" align="right">

[![crates.io](https://img.shields.io/crates/v/rigtop.svg)](https://crates.io/crates/rigtop) [![CI](https://github.com/akivajp/rigtop/actions/workflows/ci.yml/badge.svg)](https://github.com/akivajp/rigtop/actions/workflows/ci.yml)

> **Early development.** The TUI and GUI work on Linux and Windows and monitor several hosts over SSH with an overview.

A system monitor for workstations, laptops and NAS — an alternative to btop that also shows storage health, per-process GPU usage and many hosts over SSH. Two front-ends share one data-collection core:

- **`rigtop`** — terminal UI
- **`rigtop-gui`** — native GUI (no web view) with a btop-like layout and resizable panels

[日本語版 README](README.ja.md)

![rigtop TUI: CPU cores, memory, two GPUs, storage health with SMART and btrfs RAID, network, and a process list with per-process disk I/O and VRAM](docs/images/rigtop-tui.png)

## Goals

- The btop overview on one screen: CPU (total, per core, temperatures), every GPU, memory, storage, network, and a process list with kill / force kill.
- **Storage health**, which existing monitors mostly skip: btrfs RAID profiles and per-device error counters, mdadm array state, ZFS pool health, NVMe temperatures, and SMART health.
- Per-process GPU usage and VRAM (NVIDIA, via NVML), per-process disk I/O, and the container each process runs in (Docker, Podman, Kubernetes, LXC, systemd-nspawn; Linux).
- Combined CPU + GPU power draw and fan speeds (with your own labels for fan channels).
- **Multiple hosts over plain SSH**: `rigtop --agent` streams snapshots over stdin/stdout, so there is no daemon to run and no port to open.

## Roadmap

1. ~~Collection core and the TUI on Linux~~ (v0.1)
2. ~~Native GUI on Linux~~ (v0.1)
3. ~~Windows support~~ (v0.1; temperatures, fans and CPU power through LibreHardwareMonitor since v0.4; macOS on Apple Silicon checked by a user since v0.11)
4. ~~Remote monitoring over SSH~~: one host (v0.2), several hosts with an overview (v0.3)

## Installation

On Linux (x86_64 and aarch64, glibc 2.35 or newer) and macOS (Apple Silicon), the install script puts the binaries in `~/.local/bin` (no `sudo`). Run it again to update:

```sh
curl -fsSL https://github.com/akivajp/rigtop/releases/latest/download/install.sh | sh
```

It picks the archive for your machine from the latest release and checks it against `SHA256SUMS`. Add `-s -- --help` after `sh` for the options (another version, another directory, the terminal UI only). To read the script first:

```sh
curl -fsSLO https://github.com/akivajp/rigtop/releases/latest/download/install.sh
less install.sh && sh install.sh
```

**Updates**: rigtop looks for a new release on github.com at startup and every hour (it only asks which release is the latest; nothing about your machine is sent). When there is one, the title says so (`U` in the TUI, a button in the GUI, which also shows a desktop notification on Linux and macOS) and offers to install it in place of the running program; the new version runs after a restart (the GUI has a restart button). Copies from `cargo install` or a package manager are left alone, with the command to update them; on Windows, quit rigtop and run `scoop update rigtop` or replace it with the new zip. `[updates]` in the [configuration](#configuration) changes the interval or turns the check (`check = false`) or the notification (`notify = false`) off.

Or download an archive for your platform from the [Releases](https://github.com/akivajp/rigtop/releases) page and put the binaries on your `PATH`:

| Archive | Contents |
|---|---|
| `x86_64-unknown-linux-gnu` | `rigtop`, `rigtop-gui` (glibc 2.35 or newer) |
| `aarch64-unknown-linux-gnu` | `rigtop` |
| `x86_64-pc-windows-msvc` | `rigtop.exe`, `rigtop-gui.exe` |
| `aarch64-apple-darwin` | `rigtop`, `rigtop-gui` (unsigned) |

`SHA256SUMS` lists the checksums. With a Rust toolchain (1.95 or newer) you can also install from [crates.io](https://crates.io/crates/rigtop):

```sh
cargo install rigtop rigtop-gui
```

On Debian and Ubuntu (22.04 or newer), the Releases page also has `.deb` packages (`amd64` with the GUI and a menu entry, `arm64` with the terminal UI):

```sh
sudo apt install ./rigtop_*_amd64.deb
```

On Windows with [Scoop](https://scoop.sh), which also adds rigtop-gui to the Start menu (`scoop update rigtop` updates it):

```powershell
scoop install https://raw.githubusercontent.com/akivajp/rigtop/main/packaging/scoop/rigtop.json
```

With [Homebrew](https://brew.sh) (macOS on Apple Silicon, Linux), from the [akivajp/tap](https://github.com/akivajp/homebrew-tap) tap (`brew upgrade rigtop` updates it):

```sh
brew install akivajp/tap/rigtop
```

On Arch Linux and derivatives, [`packaging/aur/rigtop/PKGBUILD`](packaging/aur/rigtop/PKGBUILD) builds a package (it will be on the AUR once account registration there reopens):

```sh
git clone https://github.com/akivajp/rigtop.git
cd rigtop/packaging/aur/rigtop && makepkg -si
```

### Uninstall

| Installed with | Remove with |
|---|---|
| The install script | `rm ~/.local/bin/rigtop ~/.local/bin/rigtop-gui` |
| `.deb` | `sudo apt remove rigtop` |
| Scoop | `scoop uninstall rigtop` |
| Homebrew | `brew uninstall rigtop` |
| `cargo install` | `cargo uninstall rigtop rigtop-gui` |
| PKGBUILD | `sudo pacman -R rigtop` |
| An archive | Delete the files you extracted |

Then, if you set them up: `sudo rigtop --uninstall-collector` removes the [SMART collector](#smart-and-btrfs-scrub-results-optional) and its files, and the udev rule for [CPU power](#cpu-power-rapl) is `/etc/udev/rules.d/90-rigtop-rapl.rules`. Settings are in `~/.config/rigtop/` (Windows: `%APPDATA%\rigtop\`), and the GUI keeps its window layout in `~/.local/share/rigtop/` (macOS: `~/Library/Application Support/rigtop/`, Windows: `%APPDATA%\rigtop\data\`). LibreHardwareMonitor, if the GUI installed it for you: `winget uninstall LibreHardwareMonitor.LibreHardwareMonitor`.

## Usage

```sh
cargo run --release -p rigtop            # terminal UI
cargo run --release -p rigtop -- --help  # options (--interval-ms, --config, --dump-json)
```

`--demo` (TUI and GUI) shows made-up data for three hosts instead, to try the interface; the screenshots here use it.

| Key | Action |
|---|---|
| `↑` `↓` `PgUp` `PgDn` `Home` `End` | select a process |
| `←` `→` / `r` | change the sort column / reverse the order |
| `/` | filter by name, command, user, container or pid (`Enter` keeps it, `Esc` clears it) |
| `e` | tree view: processes under their parents, siblings in the sort order (a filter keeps the parents of what it finds) |
| `t` | show or hide kernel threads |
| `v` | show or hide boxes of the host view: `m` memory, `g` GPU, `s` storage, `n` network, `p` processes (saved as `[tui] hidden_panels`), and `w` the widget column; the others take the room |
| `Enter` | details of the selected process: its parents, user, container, start time, usage and whole command line (`Enter` or `Esc` closes) |
| `k` / `K` | send SIGTERM / SIGKILL to the selected process (asks first) |
| `n` | switch the network interface |
| `+` / `-` | slower / faster updates |
| `U` | update this machine's rigtop to a new release, when the title says one is out (asks first; runs after a restart) |
| `u` | update rigtop on this remote host to your version, when it runs an older one (asks first) |
| `a` | alert history: the alerts of every host as they appeared and cleared, kept between runs (`↑` `↓` scroll, `a` or `Esc` closes). New ones are counted in the tab bar, or in the CPU box title with one host |
| `g` | time the graphs cover: fit to the width (one sample per dot), or the last 1, 5, 15 minutes, 1, 6 or 24 hours (beyond an hour from per-minute averages) |
| `q` | quit |
| mouse | click a process to select it, click a column header to sort (again to reverse), wheel to scroll |

### GUI

![rigtop-gui: the same panels in a native window, with smooth graphs and meters, resizable and rearrangeable, here with the process tree](docs/images/rigtop-gui.png)

```sh
cargo run --release -p rigtop-gui            # same options: --interval-ms, --config
```

Drag panel borders to resize and panel titles to rearrange. Click a column header to sort, tick "tree" to show processes under their parents, double-click a process (or right-click → "Details…") for its details, right-click a process to terminate or kill it (asks first), and pick the network interface from the list.

Zoom with Ctrl + mouse wheel, Ctrl + `+` / `-` / `0`, or the `A-` / `A+` buttons in the CPU panel title. The zoom level and panel layout are restored on the next start; "panels…" shows or hides each panel (the others take the room), and "reset layout" brings back the default arrangement with every panel. The "graph" button next to them sets the time the graphs cover (fit to the width, or the last 1, 5, 15 minutes, 1, 6 or 24 hours), also restored on the next start. "alerts…" lists the alerts of every host as they appeared and cleared, kept between runs (in `~/.local/state/rigtop/`); it shows the number of new ones, in their color, until you look. "sensors…" under the CPU cores (or a click on a sensor's name) opens a window listing every temperature and fan, to name or hide them; it saves `[sensors]` in the [configuration](#configuration) file.

When a new alert appears on any host (drive errors, a missing RAID device, failing SMART, a nearly full filesystem, a failed systemd service), the GUI shows a desktop notification, also while it is minimized (Linux and macOS; not on Windows yet). Alerts present at startup are not announced. `[notifications] level` in the [configuration](#configuration) picks `"critical"` (default), `"warning"` (also resyncs, filling filesystems, hot CPUs, GPUs and drives) or `"off"`; the thresholds are in `[alerts]`; `rigtop-gui --test-notification` shows a sample notification.

With mouse support on, most terminals select text with Shift+drag.

## Widgets

Anything else you want on the same screen can go in a column of widgets on the right, next to the overview and the host views (in the TUI when the terminal is at least 120 columns wide). Hide and show it with "widgets" or "panels…" in the GUI, and `w` (or `v` then `w`) in the TUI, which keeps the choice as `[tui] show_widgets`. A widget is a built-in clock or any command, run every few seconds on the machine that displays it:

```toml
[[widgets]]
name = "Clock"
builtin = "clock"

[[widgets]]
name = "Rates"
command = "~/bin/rates.sh"   # run with sh -c (cmd /C on Windows)
interval_secs = 60           # default 5
timeout_secs = 10            # default 10
```

The command prints plain text (one row per line) or JSON rows, every field optional:

```json
{"rows": [
  {"label": "5h", "value": "42% · resets 14:30", "gauge": 0.42, "level": "ok"},
  {"value": "rigtop", "heading": true},
  {"label": "busy", "value": "2 subagents", "level": "warn"}
]}
```

`gauge` is a fraction drawn as a bar (green to red along its length, or in the row's color with `"gauge_style": "level"`, for amounts left), `level` one of `normal`, `dim`, `ok`, `warn`, `bad`. When a run fails, the widget keeps the last rows and shows the last line of the command's stderr.

Example: [`contrib/widgets/claude-code/`](contrib/widgets/claude-code/) shows Claude Code's usage limits, running sessions and subagents.

## Remote monitoring over SSH

Install rigtop on the remote hosts too, then list them in the configuration file or on the command line:

```toml
# ~/.config/rigtop/config.toml
[[hosts]]
name = "zefat"                 # also the ssh destination unless `ssh` is given

[[hosts]]
name = "nas"
ssh = "admin@nas.local"
command = "/usr/local/bin/rigtop --agent"
sections = ["storage", "cpu", "network"]   # what this host's overview card shows first
```

```sh
rigtop                          # TUI: hosts from the configuration
rigtop-gui --host hermon        # GUI: plus a host from the command line (repeatable)
```

In the GUI, **hosts…** (in the tab bar, or in the CPU panel title while only this machine is shown) adds, edits and removes hosts and can test the connection first. Changes are written to the `[[hosts]]` of the configuration file, so the TUI sees them too; the rest of the file is left as it is.

With more than one host, rigtop opens on an **overview** with one card per host, plus a tab per host (this machine included) for the full detail view. Cards show alerts (storage errors, failing SMART, degraded RAID, nearly full filesystems, hot CPUs, GPUs and drives) and then CPU, GPU, memory, the top processes, network and storage, in that order and as far as the card has room. Change the default order with `[overview] sections`, per host with `sections`, or in the GUI with each card's "sections" menu. Whether this machine gets a card is `[overview] include_local` (or the checkbox in the GUI, `l` in the TUI). Cards of hosts that need attention come first: unreachable or with a critical alert, then with warnings; the others keep their order (`[overview] alerts_first = false` keeps the configured order). In the TUI, `Tab` and `0`-`9` switch between the overview and the hosts.

![rigtop-gui showing an overview of three hosts: a workstation training on two GPUs, a laptop on battery, and a NAS flagged red for a drive with reallocated sectors, with a RAID resync and a nearly full filesystem; a clock and Claude Code widget on the right](docs/images/rigtop-gui-overview.png)

rigtop starts `rigtop --agent` on each remote host through your system's `ssh` (the built-in OpenSSH client on Windows), so `~/.ssh/config`, keys and `ProxyJump` work as usual. There is no daemon to run and no port to open; the agent exits when the connection closes, and rigtop reconnects by itself. While a host cannot be reached, its card, tab and title say for how long and since when (`offline for 5m (since 14:03)`). Hosts not shown in detail only send their top processes.

- Authentication must work without a prompt (key or agent), and the host key must already be known: connect once with `ssh <host>` first.
- Non-interactive SSH sessions often lack `~/.local/bin` (where the install script puts rigtop) and `~/.cargo/bin` on their `PATH` (zsh reads only `~/.zshenv` for them, bash usually nothing). When `rigtop` is not found there, rigtop tries `~/.local/bin/rigtop --agent`, then `~/.cargo/bin/rigtop --agent`, by itself. For other locations, set `command` for the host (or `--remote-command` for `--host`).
- Killing processes on a remote host uses the SSH user's permissions.
- A host whose rigtop is another version than yours says so in its title (`agent 0.8.2`); newer readings appear once it is updated. When it is older, `u` in the TUI or a click on that note in the GUI updates it to your version: after you confirm, rigtop runs the release's install script over SSH (Linux and macOS hosts with curl or wget) and replaces the rigtop the agent runs, then reconnects. Copies from `cargo install` or a package manager are left alone, with the command to update them; Windows hosts update with Scoop.

## Alerts without a window (`rigtop --watch`)

`rigtop --watch` watches this machine and the configured hosts without a screen and runs a command of yours when an alert appears or clears, so a degraded RAID, a failing drive or a host gone offline reaches you while no rigtop is open. The command gets the alert in its environment (`RIGTOP_HOST`, `RIGTOP_SEVERITY`, `RIGTOP_CHANGE` = `raised` / `cleared`, `RIGTOP_ALERT`, and `RIGTOP_MESSAGE` such as `[critical] nas: md0 (raid1) degraded`), so it can post to [ntfy](https://ntfy.sh), a chat webhook or mail:

```toml
[watch]
command = 'curl -s -d "$RIGTOP_MESSAGE" https://ntfy.sh/my-topic'
level = "critical"     # or "warning"
offline_after = "3m"   # a host unreachable this long is a critical alert
```

`rigtop --test-alert` runs the command once with a sample alert. Changes are printed with the time and kept in their own alert history, so a restart does not send them again; alerts already present when watching starts are sent. To keep it running as a systemd user service:

```ini
# ~/.config/systemd/user/rigtop-watch.service
[Service]
ExecStart=%h/.local/bin/rigtop --watch
Restart=on-failure

[Install]
WantedBy=default.target
```

```sh
systemctl --user enable --now rigtop-watch
loginctl enable-linger "$USER"            # keep it running while logged out
journalctl --user -u rigtop-watch -f      # what it sees and sends
```

A service has no SSH agent: remote hosts need a key without a passphrase (or `SSH_AUTH_SOCK` pointing at an agent). Run it on one machine only, or each one sends the same alerts.

## Linux

Most readings need neither extra software nor privileges: CPU, memory, processes, temperatures and fans (hwmon), network, disks, btrfs and md RAID, and laptop batteries. A few need a package or a permission:

| Reading | Needs | Debian / Ubuntu | Arch / CachyOS |
|---|---|---|---|
| NVIDIA GPUs | the driver's NVML library | comes with the NVIDIA driver | `nvidia-utils` |
| AMD / Intel GPUs | nothing (sysfs); Intel's utilization is not available without privileges | | |
| Wi-Fi signal and rate | `iw` | `sudo apt install iw` | `sudo pacman -S iw` |
| SMART | `smartmontools` and a timer ([below](#smart-and-btrfs-scrub-results-optional)) | `sudo apt install smartmontools` | `sudo pacman -S smartmontools` |
| CPU power | read access to RAPL (below) | | |

### CPU power (RAPL)

Since Linux 5.10 the CPU energy counters (`/sys/class/powercap/intel-rapl:*/energy_uj`, named so on AMD too) are readable only by root, because fine-grained power readings can leak secrets ([PLATYPUS](https://platypusattack.com/), CVE-2020-8694). A udev rule can let your admin group read them; replace `ADMIN_GROUP` with `sudo` on Debian/Ubuntu or `wheel` on Arch:

```sh
echo 'SUBSYSTEM=="powercap", KERNEL=="intel-rapl:*", RUN+="/usr/bin/chgrp ADMIN_GROUP /sys%p/energy_uj", RUN+="/usr/bin/chmod g+r /sys%p/energy_uj"' \
  | sudo tee /etc/udev/rules.d/90-rigtop-rapl.rules
sudo udevadm control --reload && sudo udevadm trigger --subsystem-match=powercap
```

Without it, AMD laptops show the APU's SoC power (`SoC`) instead, and other machines show `--`.

## Windows

Both `rigtop` and `rigtop-gui` run on Windows (see [Installation](#installation)). Builds of the latest `main` are also attached to the [CI runs](https://github.com/akivajp/rigtop/actions/workflows/ci.yml) (artifact `rigtop-windows-x86_64`).

What differs from Linux:

- CPU load, memory, processes, network (with link speeds), drive capacity and per-drive I/O, and NVIDIA GPUs (via `nvml.dll` from the driver) work without extra software.
- Temperatures, fans, CPU power and drive health (wear, spare capacity, temperature) need hardware drivers that Windows does not ship. rigtop reads them from [LibreHardwareMonitor](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor) when it is running (see below); otherwise only what Windows itself reports is shown, which is often nothing. Drives are named `disk0`, `disk1`... as in Disk Management.
- There is no load average and no disk busy percentage.
- Windows has no signals: "kill" (`K`) ends the process immediately; "terminate" (`k`) asks the process's windows to close, like Task Manager's "End task", and reports when the process has no window (console programs, services).

![rigtop-gui on Windows with CPU temperatures, fans and CPU power read through LibreHardwareMonitor, an NVIDIA GPU, drives and network](docs/images/rigtop-gui-windows.png)

### Temperatures, fans and CPU power with LibreHardwareMonitor

In `rigtop-gui`, **set up…** next to the note in the CPU panel does steps 1 to 3 for you: it installs LibreHardwareMonitor with winget if needed, turns its web server on for this computer only (127.0.0.1) and starts it. By hand:

1. Install it, together with the PawnIO driver it needs for CPU sensors (Windows asks for administrator approval for the driver):
   ```powershell
   winget install LibreHardwareMonitor.LibreHardwareMonitor
   ```
   (Or download `LibreHardwareMonitor.zip` from its [releases](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor/releases), unblock it in the file's Properties, and extract it.)
2. Run `LibreHardwareMonitor` from a new terminal (or the extracted `LibreHardwareMonitor.exe`) and allow it to run as administrator.
3. Turn on **Options → Remote Web Server → Run** (port 8085). rigtop reads `http://127.0.0.1:8085/data.json` and picks the values up within 10 seconds; until then, the CPU panel shows a note.
4. Optionally enable **Options → Run On Windows Startup** and **Start Minimized**.

The web server must not require authentication. Its port can be reached from other machines unless Windows Firewall blocks it. For another address or port, set `[lhm] url` in the [configuration](#configuration); `url = ""` turns this off. NVIDIA GPUs keep coming from the driver, not LibreHardwareMonitor.

## SMART and btrfs scrub results (optional)

SMART data and btrfs scrub results are readable only by root, and rigtop never runs as root or loosens device permissions. Instead, a root systemd timer collects them every 10 minutes into world-readable files under `/var/lib/rigtop/`, and rigtop reads those:

- `smart/<device>.json`: `smartctl -j -a` for every drive (sleeping disks are not woken up)
- `btrfs/scrub.status.<fsid>`: copies of btrfs-progs' scrub status, shown as when data was last verified

```sh
sudo apt install smartmontools   # Arch: sudo pacman -S smartmontools
sudo "$(command -v rigtop)" --install-collector
```

`--install-collector` writes the script and the systemd units (the files in [`contrib/collect/`](contrib/collect/)), starts the timer and collects once right away; `--uninstall-collector` removes them and the collected files. The full path is needed because `sudo` usually does not search `~/.cargo/bin`. From a checkout, the same by hand:

```sh
sudo install -m 755 contrib/collect/rigtop-collect /usr/local/libexec/
sudo install -m 644 contrib/collect/rigtop-collect.service contrib/collect/rigtop-collect.timer /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now rigtop-collect.timer
```

The SMART files include drive serial numbers.

## Configuration

`~/.config/rigtop/config.toml` (`%APPDATA%\rigtop\config.toml` on Windows). Every setting is optional. Edits apply while rigtop runs, within a second: hosts added to `[[hosts]]` connect and removed ones disconnect (the others keep their history), and widgets restart. If the file cannot be read, rigtop says so and keeps the settings it has.

```toml
[sensors]
# Names for sensor channels, keyed by the channel key rigtop shows.
labels = { "nct6686:nct6683.2592/fan1" = "CPU fan" }
# Channels to hide, e.g. unconnected thermistors reading -40 °C.
hidden = ["nct6686:nct6683.2592/temp4"]

[smart]
dir = "/var/lib/rigtop/smart"

[btrfs]
scrub_dir = "/var/lib/rigtop/btrfs"

[alerts]
# When readings become alerts (0 turns one off).
filesystem_warning_percent = 90
filesystem_critical_percent = 97
cpu_warning_celsius = 96    # hottest CPU reading
gpu_warning_celsius = 85
drive_warning_celsius = 70  # SMART or NVMe sensor
ignored_units = []          # failed systemd units not to warn about, e.g. ["foo.service"]

[notifications]
# rigtop-gui: desktop notifications for new alerts ("critical", "warning" or "off").
level = "critical"

[updates]
# Look for new releases on github.com, at startup and every interval
# ("30m", "6h", "1d", "1h30m", ...; at least a minute).
check = true
interval = "1h"
notify = true   # rigtop-gui: desktop notification for a new release

[watch]
# rigtop --watch: a command run when an alert appears or clears on any host.
command = ""           # e.g. 'curl -s -d "$RIGTOP_MESSAGE" https://ntfy.sh/my-topic'
level = "critical"     # or "warning" ("off" runs nothing)
cleared = true         # also when an alert goes away
interval = "10s"       # time between samples of each host
offline_after = "3m"   # a host unreachable this long is a critical alert ("0s" turns it off)
timeout = "30s"        # a run taking longer is stopped

[tui]
# Boxes of the host view not to draw (also toggled with `v`).
hidden_panels = []   # e.g. ["gpu", "network"]
show_widgets = true  # the widget column (`w`)

[lhm]
# Windows: LibreHardwareMonitor's web server ("" to disable).
url = "http://127.0.0.1:8085/data.json"
```

## Code signing policy

Code signing of the Windows releases through the [SignPath Foundation](https://signpath.org) has been applied for; until it is approved, releases are unsigned. Once approved: free code signing provided by [SignPath.io](https://about.signpath.io), certificate by [SignPath Foundation](https://signpath.org).

- **What is signed**: `rigtop.exe` and `rigtop-gui.exe` in the Windows release archive, built by GitHub Actions ([`release.yml`](.github/workflows/release.yml)) from the tagged source of this repository. Nothing built elsewhere is signed, and every signing request is approved by hand.
- **Team**: [Akiva Miura (@akivajp)](https://github.com/akivajp) is the author, reviewer and approver. Changes from others come as pull requests and are reviewed before they are merged.

### Privacy

rigtop collects no telemetry. It connects only to:

- the hosts you list for remote monitoring, through your own `ssh`;
- LibreHardwareMonitor on this machine (Windows, `[lhm] url`);
- github.com, to see whether a new release is out, at startup and every hour. It only asks which release is the latest and sends nothing about your machine; `[updates] check = false` in the [configuration](#configuration) turns it off. An update you choose to install is downloaded from github.com.

Widgets run the commands you configure.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
