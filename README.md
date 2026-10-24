# rigtop

> **Work in progress.** The Linux TUI works; the GUI and remote monitoring are not there yet.

A btop-inspired system monitor for Linux, planned with two front-ends that share one data-collection core:

- **`rigtop`** — terminal UI
- **`rigtop-gui`** — native GUI (no web view) with a btop-like layout and resizable panels

[日本語版 README](README.ja.md)

## Goals

- The btop overview on one screen: CPU (total, per core, temperatures), every GPU, memory, storage, network, and a process list with kill / force kill.
- **Storage health**, which existing monitors mostly skip: btrfs RAID profiles and per-device error counters, mdadm array state, NVMe temperatures, and SMART health.
- Per-process GPU usage and VRAM (NVIDIA, via NVML).
- Combined CPU + GPU power draw and fan speeds (with your own labels for fan channels).
- **Multiple hosts over plain SSH**: `rigtop --agent` streams snapshots over stdin/stdout, so there is no daemon to run and no port to open.

## Roadmap

1. Collection core and the TUI on Linux
2. Native GUI on Linux
3. Windows support (macOS builds without hardware testing)
4. Remote monitoring of registered hosts over SSH

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
