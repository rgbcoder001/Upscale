use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("not implemented yet: {0}")]
    NotImplemented(&'static str),

    #[error("unknown profile '{0}' (expected anime or cg)")]
    UnknownProfile(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
