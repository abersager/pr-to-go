//! The PR to Go core: storage, GitHub access, sync, the outbox and comment
//! remapping. It has no UI dependencies so a future iPad app can reuse it.

pub mod auth;
pub mod blobstore;
pub mod browse;
pub mod checks;
pub mod clock;
pub mod commands;
pub mod db;
pub mod diff;
pub mod drafts;
pub mod error;
pub mod generated;
pub mod github;
pub mod inbox;
pub mod outbox;
pub mod remap;
pub mod service;
pub mod sync;
pub mod views;

pub use error::{Error, Result};
pub use service::{AuthStatus, Connectivity, Core, CoreOptions, Event};
