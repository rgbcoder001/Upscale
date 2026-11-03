//! Upscale engine. Replaces `reference/upscale.ps1`; see `PLAN.md` §3 for the module layout.
//!
//! This crate has no Tauri dependency. The desktop app and `upscale-cli` both drive it
//! through the same API, and progress leaves it as [`event::Event`] JSON lines.

pub mod analyze;
pub mod batch;
pub mod chunks;
pub mod claim;
pub mod cmd;
pub mod color;
pub mod disc;
pub mod disk;
pub mod drives;
pub mod error;
pub mod event;
pub mod files;
pub mod gpu;
pub mod lang;
pub mod library;
pub mod log;
pub mod pipeline;
pub mod plan;
pub mod platform;
pub mod probe;
pub mod process;
pub mod profile;
pub mod pump;
pub mod queue;
pub mod report;
pub mod rip;
pub mod settings;
pub mod tmdb;
pub mod tools;
pub mod tracks;
pub mod util;

pub use error::{Error, Result};
pub use event::Event;
pub use profile::ProfileId;

/// Version of the core crate, reported by the CLI and shown in the app.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
