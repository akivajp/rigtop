//! Data collection core shared by the rigtop TUI and GUI.
//!
//! - [`snapshot`]: the serializable measurement types exchanged between data
//!   sources and front-ends (also the wire format for remote monitoring).
//! - [`source`]: the [`DataSource`] trait and the [`LocalSource`] implementation.
//! - [`config`]: user configuration (sensor labels, SMART directory).
//! - [`system`], [`gpu`], [`smart`], `linux`: the individual collectors;
//!   [`fill`]: when filesystems will be full.
//! - [`agent`], [`remote`]: remote monitoring over SSH (`rigtop --agent` and
//!   the [`remote::RemoteSource`] that talks to it).
//! - [`demo`]: made-up hosts for trying the interface (`--demo`).
//! - [`snooze`]: alerts silenced for a while, shared by the front-ends.
//! - [`worker`], [`view`], [`format`]: front-end helpers shared by the TUI
//!   and the GUI (background polling thread, process table logic, units).

pub mod agent;
pub mod config;
pub mod demo;
pub mod fill;
pub mod format;
pub mod gpu;
pub mod hosts;
// Parsed (and tested) everywhere, used on Windows.
#[cfg(any(windows, test))]
mod lhm;
#[cfg(target_os = "linux")]
pub mod linux;
pub mod remote;
pub mod smart;
pub mod snapshot;
pub mod snooze;
pub mod source;
pub mod system;
pub mod update;
pub mod view;
pub mod widget;
#[cfg(windows)]
mod windows;
pub mod worker;

pub use config::Config;
pub use snapshot::Snapshot;
pub use source::{DataSource, LocalSource, MIN_POLL_INTERVAL, ProcessSignal, SourceError};
