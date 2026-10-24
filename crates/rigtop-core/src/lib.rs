//! Data collection core shared by the rigtop TUI and GUI.
//!
//! - [`snapshot`]: the serializable measurement types exchanged between data
//!   sources and front-ends (also the wire format for remote monitoring).
//! - [`source`]: the [`DataSource`] trait and the [`LocalSource`] implementation.
//! - [`config`]: user configuration (sensor labels, SMART directory).
//! - [`system`], [`gpu`], [`smart`], `linux`: the individual collectors.
//! - [`worker`], [`view`], [`format`]: front-end helpers shared by the TUI
//!   and the GUI (background polling thread, process table logic, units).

pub mod config;
pub mod format;
pub mod gpu;
#[cfg(target_os = "linux")]
pub mod linux;
pub mod smart;
pub mod snapshot;
pub mod source;
pub mod system;
pub mod view;
#[cfg(windows)]
mod windows;
pub mod worker;

pub use config::Config;
pub use snapshot::Snapshot;
pub use source::{DataSource, LocalSource, MIN_POLL_INTERVAL, ProcessSignal, SourceError};
