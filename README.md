# rigtop

[![crates.io](https://img.shields.io/crates/v/rigtop.svg)](https://crates.io/crates/rigtop) [![CI](https://github.com/akivajp/rigtop/actions/workflows/ci.yml/badge.svg)](https://github.com/akivajp/rigtop/actions/workflows/ci.yml)

> **Early development (v0.1).** The TUI and GUI work on Linux and Windows; remote monitoring is not there yet.

A btop-inspired system monitor with two front-ends that share one data-collection core:

- **`rigtop`** — terminal UI
- **`rigtop-gui`** — native GUI (no web view) with a btop-like layout and resizable panels

[日本語版 README](README.ja.md)

![rigtop TUI: CPU cores, memory, two GPUs, storage health with SMART and btrfs RAID, network, and a process list with per-process VRAM](docs/images/rigtop-tui.png)

## Goals

- The btop overview on one screen: CPU (total, per core, temperatures), every GPU, memory, storage, network, and a process list with kill / force kill.
- **Storage health**, which existing monitors mostly skip: btrfs RAID profiles and per-device error counters, mdadm array state, NVMe temperatures, and SMART health.
- Per-process GPU usage and VRAM (NVIDIA, via NVML).
- Combined CPU + GPU power draw and fan speeds (with your own labels for fan channels).
- **Multiple hosts over plain SSH**: `rigtop --agent` streams snapshots over stdin/stdout, so there is no daemon to run and no port to open.

## Roadmap

1. ~~Collection core and the TUI on Linux~~ (v0.1)
2. ~~Native GUI on Linux~~ (v0.1)
3. ~~Windows support~~ (v0.1; temperatures, fans and CPU power still missing; macOS builds without hardware testing)
4. Remote monitoring of registered hosts over SSH

## Installation

Download an archive for your platform from the [Releases](https://github.com/akivajp/rigtop/releases) page and put the binaries on your `PATH`:

| Archive | Contents |
|---|---|
| `x86_64-unknown-linux-gnu` | `rigtop`, `rigtop-gui` (glibc 2.35 or newer) |
| `aarch64-unknown-linux-gnu` | `rigtop` |
| `x86_64-pc-windows-msvc` | `rigtop.exe`, `rigtop-gui.exe` |
| `aarch64-apple-darwin` | `rigtop`, `rigtop-gui` (untested, unsigned) |

`SHA256SUMS` lists the checksums. With a Rust toolchain (1.95 or newer) you can also install from [crates.io](https://crates.io/crates/rigtop):

```sh
cargo install rigtop rigtop-gui
```

## Usage

```sh
cargo run --release -p rigtop            # terminal UI
cargo run --release -p rigtop -- --help  # options (--interval-ms, --config, --dump-json)
```

| Key | Action |
|---|---|
| `↑` `↓` `PgUp` `PgDn` `Home` `End` | select a process |
| `←` `→` / `r` | change the sort column / reverse the order |
| `/` | filter by name, command, user or pid (`Enter` keeps it, `Esc` clears it) |
| `t` | show or hide kernel threads |
| `k` / `K` | send SIGTERM / SIGKILL to the selected process (asks first) |
| `n` | switch the network interface |
| `+` / `-` | slower / faster updates |
| `q` | quit |
| mouse | click a process to select it, click a column header to sort (again to reverse), wheel to scroll |

### GUI

![rigtop-gui: the same panels in a native window, with smooth graphs and meters, resizable and rearrangeable](docs/images/rigtop-gui.png)

```sh
cargo run --release -p rigtop-gui            # same options: --interval-ms, --config
```

Drag panel borders to resize and panel titles to rearrange. Click a column header to sort, right-click a process to terminate or kill it (asks first), and pick the network interface from the list.

Zoom with Ctrl + mouse wheel, Ctrl + `+` / `-` / `0`, or the `A-` / `A+` buttons in the CPU panel title. The zoom level and panel layout are restored on the next start; "reset layout" brings back the default arrangement.

With mouse support on, most terminals select text with Shift+drag.

## Windows

Both `rigtop` and `rigtop-gui` run on Windows (see [Installation](#installation)). Builds of the latest `main` are also attached to the [CI runs](https://github.com/akivajp/rigtop/actions/workflows/ci.yml) (artifact `rigtop-windows-x86_64`).

What differs from Linux:

- CPU load, memory, processes, network (with link speeds), drive capacity and per-drive I/O, and NVIDIA GPUs (via `nvml.dll` from the driver) work without extra software.
- Temperatures, fans and CPU power need hardware drivers that Windows does not ship; only what Windows itself reports is shown, which is often nothing. (Reading them through LibreHardwareMonitor is planned.)
- There is no load average and no disk busy percentage.
- Windows has no signals: "kill" (`K`) ends the process immediately; "terminate" (`k`) asks the process's windows to close, like Task Manager's "End task", and reports when the process has no window (console programs, services).

## SMART and btrfs scrub results (optional)

SMART data and btrfs scrub results are readable only by root, and rigtop never runs as root or loosens device permissions. Instead, a root systemd timer collects them every 10 minutes into world-readable files under `/var/lib/rigtop/`, and rigtop reads those:

- `smart/<device>.json`: `smartctl -j -a` for every drive (sleeping disks are not woken up)
- `btrfs/scrub.status.<fsid>`: copies of btrfs-progs' scrub status, shown as when data was last verified

```sh
sudo apt install smartmontools   # or your distribution's package
sudo install -m 755 contrib/collect/rigtop-collect /usr/local/libexec/
sudo install -m 644 contrib/collect/rigtop-collect.service contrib/collect/rigtop-collect.timer /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now rigtop-collect.timer
```

The SMART files include drive serial numbers.

## Configuration

`~/.config/rigtop/config.toml` (`%APPDATA%\rigtop\config.toml` on Windows). Every setting is optional.

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
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
