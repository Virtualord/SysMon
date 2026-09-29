//! # syswatch
//!
//! A fast, lightweight native terminal system monitor for Linux.
//!
//! The crate is split into a library and a thin binary so that the collection and
//! presentation logic can be exercised by integration tests:
//!
//! * [`collector`] – reads metrics from `sysinfo`, `/proc` and `/sys`.
//! * [`app`] – central application state, actions and view routing.
//! * [`events`] – background monitoring thread and the channel used to ship samples.
//! * [`ui`] – pure rendering code (no I/O, no side effects).
//! * [`terminal`] – RAII terminal guard, panic hook and signal handling.
//! * [`config`] – TOML configuration loading and validation.
//!
//! Rendering is a pure function of [`app::App`]: every widget is rebuilt each frame
//! from state that the monitoring thread hands over through a channel, so the main
//! thread never blocks on data collection.

pub mod app;
pub mod collector;
pub mod config;
pub mod events;
pub mod format;
pub mod history;
pub mod terminal;
pub mod ui;

pub use app::App;

/// Version of the application, taken from the crate manifest.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Name of the application binary.
pub const NAME: &str = env!("CARGO_PKG_NAME");
