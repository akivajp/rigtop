# rigtop

> **Work in progress.** Nothing usable yet.

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

## SMART health (optional)

Reading SMART data requires root, and rigtop never runs as root or loosens device permissions. Instead, a root systemd timer dumps `smartctl -j -a` for every drive into `/var/lib/rigtop/smart/`, and rigtop reads those files. Sleeping disks are not woken up.

```sh
sudo apt install smartmontools   # or your distribution's package
sudo install -m 755 contrib/smart/rigtop-smart-collect /usr/local/libexec/
sudo install -m 644 contrib/smart/rigtop-smart.service contrib/smart/rigtop-smart.timer /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now rigtop-smart.timer
```

The result files are world-readable and include drive serial numbers.

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
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
