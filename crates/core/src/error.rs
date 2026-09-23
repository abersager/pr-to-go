use crate::github::GhError;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("GitHub: {0}")]
    GitHub(#[from] GhError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
    #[error("not signed in")]
    NotAuthenticated,
    #[error("{0}")]
    Internal(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Internal(format!("JSON: {e}"))
    }
}
