//! GitHub API access: GraphQL first, REST where GraphQL has gaps.

pub mod client;
pub mod queries;

pub use client::{GitHub, GitHubConfig, OpKind, RateBudget, Viewer};

/// How a GitHub request failed. The outbox depends on the distinction between
/// `Offline` (the request never reached GitHub) and `Ambiguous` (it may have
/// been applied), so every mutation failure must land in the right one.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GhError {
    /// The request didn't reach GitHub: no network, DNS failure, or a captive
    /// portal answered instead.
    #[error("offline: {0}")]
    Offline(String),
    /// The request may or may not have been applied (timeout after sending,
    /// connection reset, 5xx on a mutation). Reconcile before retrying.
    #[error("outcome unknown: {0}")]
    Ambiguous(String),
    /// 5xx on a read. Safe to retry.
    #[error("GitHub server error ({0})")]
    Server(u16),
    #[error("rate limited, retry after {retry_after_s}s")]
    RateLimited { retry_after_s: u64, secondary: bool },
    #[error("GitHub rejected the token (401)")]
    Unauthorized,
    #[error("not signed in")]
    NoToken,
    #[error("forbidden: {message}")]
    Forbidden { message: String, sso_url: Option<String> },
    #[error("not found")]
    NotFound,
    /// 422 or a GraphQL UNPROCESSABLE error: the request was understood and
    /// refused (for example a line that isn't in the diff).
    #[error("rejected by GitHub: {0}")]
    Unprocessable(String),
    #[error("unexpected response from GitHub: {0}")]
    Protocol(String),
}

impl GhError {
    /// Worth retrying later without user action.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            GhError::Offline(_) | GhError::Ambiguous(_) | GhError::Server(_) | GhError::RateLimited { .. }
        )
    }
}
