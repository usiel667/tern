//! TCP + TLS (implicit or STARTTLS) + authentication.


use async_imap::{Authenticator, Client};
use tern_auth::{Credential, xoauth2_payload};
use tern_core::account::{Security, ServerConfig};
use tern_core::{Error, Result};
use tern_tls::Start;

pub type Stream = tern_tls::Stream;
pub type Session = async_imap::Session<Stream>;

pub(crate) fn net(e: impl std::fmt::Display) -> Error {
    Error::Network(e.to_string())
}

pub(crate) fn proto(e: impl std::fmt::Display) -> Error {
    Error::Protocol(e.to_string())
}

/// Classify an error from an IMAP command. A dead connection is a network
/// problem (worth retrying later); anything the server said (NO, BAD) or sent
/// that we couldn't use is a protocol problem (retrying won't help).
pub(crate) fn cmd(e: async_imap::error::Error) -> Error {
    match e {
        async_imap::error::Error::Io(_) | async_imap::error::Error::ConnectionLost => net(e),
        e => proto(e),
    }
}

/// Open an authenticated IMAP session.
pub async fn connect(server: &ServerConfig, cred: &Credential) -> Result<Session> {
    let start = match server.security {
        Security::Tls => Start::Implicit,
        Security::StartTls => Start::ImapStartTls,
    };
    // Certificate problems come back as Error::Certificate (retried like a network error).
    let trusted = server.trusted_cert.as_deref();
    let stream = tern_tls::open(&server.host, server.port, start, trusted).await.map_err(|e| e.into_error(trusted))?;
    let mut client = Client::new(stream);
    if start == Start::Implicit {
        client
            .read_response()
            .await
            .map_err(net)?
            .ok_or_else(|| Error::Network("server closed before greeting".into()))?;
    }

    match cred {
        Credential::Password { username, password } => client
            .login(username, password)
            .await
            .map_err(|(e, _)| Error::Auth(e.to_string())),
        Credential::OAuth2 { username, access_token } => client
            .authenticate(
                "XOAUTH2",
                XOAuth2 {
                    payload: xoauth2_payload(username, access_token),
                    sent: false,
                },
            )
            .await
            .map_err(|(e, _)| Error::Auth(e.to_string())),
        Credential::Bearer { .. } => Err(Error::Unsupported("bearer tokens over IMAP")),
    }
}

struct XOAuth2 {
    payload: String,
    sent: bool,
}

impl Authenticator for XOAuth2 {
    type Response = String;

    fn process(&mut self, _challenge: &[u8]) -> String {
        // On failure the server sends a JSON error as a second challenge; the
        // client must answer with an empty line to receive the tagged NO.
        if self.sent {
            String::new()
        } else {
            self.sent = true;
            self.payload.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_imap::error::Error as ImapError;

    #[test]
    fn dead_connection_is_network_server_refusal_is_protocol() {
        let io = std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset");
        assert!(matches!(cmd(ImapError::Io(io)), Error::Network(_)));
        assert!(matches!(cmd(ImapError::ConnectionLost), Error::Network(_)));
        assert!(matches!(cmd(ImapError::No("[TRYCREATE] no such mailbox".into())), Error::Protocol(_)));
        assert!(matches!(cmd(ImapError::Bad("syntax".into())), Error::Protocol(_)));
        assert!(matches!(cmd(ImapError::Append), Error::Protocol(_)));
    }
}
