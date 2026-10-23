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

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
