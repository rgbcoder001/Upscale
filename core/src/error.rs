use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("not implemented yet: {0}")]
    NotImplemented(&'static str),

    #[error("unknown profile '{0}' (expected anime or cg)")]
    UnknownProfile(String),

    #[error("settings: {0}")]
    Settings(String),

    #[error("{tool} not found{hint}")]
    ToolNotFound { tool: &'static str, hint: String },

    /// A tool is missing, broken or failed its self-test; the message says what to do.
    #[error("{0}")]
    Tool(String),

    #[error("{tool} failed on {}: {message}", file.display())]
    Probe {
        tool: &'static str,
        file: PathBuf,
        message: String,
    },

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
