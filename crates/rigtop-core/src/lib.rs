//! Data collection core shared by the rigtop TUI and GUI.
//!
//! - [`snapshot`]: the serializable measurement types exchanged between data
//!   sources and front-ends (also the wire format for remote monitoring).
//! - [`source`]: the [`DataSource`] trait and the [`LocalSource`] implementation.
//! - [`system`], [`gpu`], `linux`: the individual collectors.

pub mod gpu;
#[cfg(target_os = "linux")]
pub mod linux;
pub mod snapshot;
pub mod source;
pub mod system;

pub use snapshot::Snapshot;
pub use source::{DataSource, LocalSource, MIN_POLL_INTERVAL, SourceError};
