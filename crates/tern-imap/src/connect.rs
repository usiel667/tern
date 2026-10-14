//! TCP + TLS (implicit or STARTTLS) + authentication.

use std::sync::Arc;

use async_imap::{Authenticator, Client};
use tern_auth::{Credential, xoauth2_payload};
use tern_core::account::{Security, ServerConfig};
use tern_core::{Error, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::{self, ClientConfig, RootCertStore, pki_types::ServerName};

pub type Stream = TlsStream<TcpStream>;
pub type Session = async_imap::Session<Stream>;

pub(crate) fn net(e: impl std::fmt::Display) -> Error {
    Error::Network(e.to_string())
}

pub(crate) fn proto(e: impl std::fmt::Display) -> Error {
    Error::Protocol(e.to_string())
}

fn tls_connector() -> TlsConnector {
    // Mozilla's roots plus the system store (/etc/ssl/certs on Arch), so
    // self-hosted servers signed by a locally trusted CA work too.
    let mut roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let native = rustls_native_certs::load_native_certs();
    let (added, _ignored) = roots.add_parsable_certificates(native.certs);
    tracing::debug!(added, "loaded system root certificates");
    // Explicit provider: other crates in the tree may enable aws-lc-rs too, which
    // makes the process-default lookup ambiguous.
    let config = ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("ring supports default protocol versions")
    .with_root_certificates(roots)
    .with_no_client_auth();
    TlsConnector::from(Arc::new(config))
}

/// Open an authenticated IMAP session.
pub async fn connect(server: &ServerConfig, cred: &Credential) -> Result<Session> {
    let mut tcp = TcpStream::connect((server.host.as_str(), server.port))
        .await
        .map_err(net)?;
    let name = ServerName::try_from(server.host.clone()).map_err(|e| Error::Config(e.to_string()))?;
    let tls = tls_connector();

    let client = match server.security {
        Security::Tls => {
            let stream = tls.connect(name, tcp).await.map_err(net)?;
            let mut client = Client::new(stream);
            client
                .read_response()
                .await
                .map_err(net)?
                .ok_or_else(|| Error::Network("server closed before greeting".into()))?;
            client
        }
        Security::StartTls => {
            read_line(&mut tcp).await?; // greeting
            tcp.write_all(b"T0 STARTTLS\r\n").await.map_err(net)?;
            loop {
                let line = read_line(&mut tcp).await?;
                if let Some(rest) = line.strip_prefix("T0 ") {
                    if !rest.to_ascii_uppercase().starts_with("OK") {
                        return Err(Error::Protocol(format!("STARTTLS refused: {rest}")));
                    }
                    break;
                }
            }
            // No new greeting after STARTTLS (RFC 3501 §6.2.1).
            Client::new(tls.connect(name, tcp).await.map_err(net)?)
        }
    };

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

/// Byte-at-a-time line read, only used for the few pre-TLS lines so nothing
/// gets buffered past the STARTTLS response.
async fn read_line(tcp: &mut TcpStream) -> Result<String> {
    let mut line = Vec::new();
    loop {
        let b = tcp.read_u8().await.map_err(net)?;
        if b == b'\n' {
            break;
        }
        if line.len() > 8192 {
            return Err(Error::Protocol("line too long".into()));
        }
        line.push(b);
    }
    Ok(String::from_utf8_lossy(&line).trim_end().to_owned())
}
