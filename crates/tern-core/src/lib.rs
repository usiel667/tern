//! Core domain types shared by every Tern crate.
//!
//! Nothing in here does I/O. Protocol crates (`tern-imap`, `tern-jmap`) implement
//! [`backend::MailBackend`]; `tern-store` persists the model; `tern-sync` glues them.

pub mod account;
pub mod backend;
pub mod compose;
pub mod error;
pub mod mime;
pub mod model;
pub mod search;
pub mod threading;

pub use error::{Error, Result};
