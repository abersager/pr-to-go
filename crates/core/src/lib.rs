//! The PR to Go core: storage, GitHub access, sync, the outbox and comment
//! remapping. It has no UI dependencies so a future iPad app can reuse it.

pub mod clock;
pub mod db;
pub mod error;
pub mod github;

pub use error::{Error, Result};
