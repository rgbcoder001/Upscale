//! Upscale engine. Replaces `reference/upscale.ps1`; see `PLAN.md` §3 for the module layout.
//!
//! This crate has no Tauri dependency. The desktop app and `upscale-cli` both drive it
//! through the same API, and progress leaves it as [`event::Event`] JSON lines.

pub mod error;
pub mod event;
pub mod profile;

pub use error::{Error, Result};
pub use event::Event;
pub use profile::ProfileId;

/// Version of the core crate, reported by the CLI and shown in the app.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
