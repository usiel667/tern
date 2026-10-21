use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("network error: {0}")]
    Network(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("not supported by this server: {0}")]
    Unsupported(&'static str),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("configuration error: {0}")]
    Config(String),
    /// The server refused this message permanently (too large, recipient refused, policy);
    /// sending it again won't help.
    #[error("{0}")]
    Rejected(String),
}

pub type Result<T> = std::result::Result<T, Error>;
