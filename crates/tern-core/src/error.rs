use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("network error: {0}")]
    Network(String),
    /// The server's certificate isn't trusted by this computer and isn't the one the account
    /// trusts (it changed, or was never trusted). Retried like a network error, and shown so
    /// the person can decide.
    #[error("{}", .0.message)]
    Certificate(Box<CertIssue>),
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

/// An untrusted server certificate, as the server presented it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertIssue {
    pub host: String,
    pub port: u16,
    /// SHA-256 of the certificate presented now, "AB:CD:…".
    pub fingerprint: String,
    /// The fingerprint the account trusted until now, if any.
    pub trusted: Option<String>,
    /// What's wrong, in plain words.
    pub message: String,
}

impl Error {
    /// Couldn't reach the server (or couldn't trust it): the operation is worth retrying later.
    pub fn is_network(&self) -> bool {
        matches!(self, Error::Network(_) | Error::Certificate(_))
    }
}

pub type Result<T> = std::result::Result<T, Error>;
