//! TLS for Tern's IMAP and SMTP connections.
//!
//! * Certificates are checked against Mozilla's roots plus the system store, as before.
//! * When a check fails, the problem is named in plain words (expired, wrong server name,
//!   self-signed…) instead of rustls's error text, and the certificate's SHA-256 fingerprint
//!   is kept so the person can choose to trust it.
//! * A server can carry a trusted fingerprint (`ServerConfig::trusted_cert`): that exact
//!   certificate is then accepted even though the system doesn't trust it, the way a browser
//!   exception works. The server still has to prove it holds the certificate's key.
//!
//! Plain TCP setup is here too (implicit TLS, or STARTTLS for IMAP and SMTP), so the account
//! setup can look at a server's certificate before signing in.

use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::client::WebPkiServerVerifier;
use tokio_rustls::rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio_rustls::rustls::{self, CertificateError, ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};

pub type Stream = TlsStream<TcpStream>;

/// How long to wait for the server before giving up (connecting, STARTTLS, handshake).
const TIMEOUT: Duration = Duration::from_secs(30);

/// What's wrong with a server's certificate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    Expired,
    NotYetValid,
    /// Issued for another host name.
    WrongHost,
    SelfSigned,
    /// Signed by an authority this computer doesn't trust (often a private CA or self-signed).
    UnknownIssuer,
    Revoked,
    Other(String),
}

impl Problem {
    /// The certificate problem in a rustls error, if it is one.
    pub fn from_rustls(e: &rustls::Error) -> Option<Problem> {
        let rustls::Error::InvalidCertificate(c) = e else { return None };
        Some(match c {
            CertificateError::Expired | CertificateError::ExpiredContext { .. } => Problem::Expired,
            CertificateError::NotValidYet | CertificateError::NotValidYetContext { .. } => Problem::NotYetValid,
            CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. } => Problem::WrongHost,
            CertificateError::UnknownIssuer => Problem::UnknownIssuer,
            CertificateError::Revoked => Problem::Revoked,
            // webpki's name for a self-signed certificate that says it's an authority.
            CertificateError::Other(o) if o.to_string().contains("CaUsedAsEndEntity") => Problem::SelfSigned,
            other => Problem::Other(format!("{other:?}")),
        })
    }

    /// Finishes "The certificate …".
    fn words(&self) -> String {
        match self {
            Problem::Expired => "has expired".into(),
            Problem::NotYetValid => "isn’t valid yet (is this computer’s clock right?)".into(),
            Problem::WrongHost => "was issued for a different server name".into(),
            Problem::SelfSigned => "is self-signed".into(),
            Problem::UnknownIssuer => {
                "isn’t signed by an authority this computer trusts (it may be self-signed or from a private authority)"
                    .into()
            }
            Problem::Revoked => "has been revoked by whoever issued it".into(),
            Problem::Other(why) => format!("couldn’t be checked ({why})"),
        }
    }
}

/// A server whose certificate this computer doesn't trust, as it presented itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Untrusted {
    pub host: String,
    pub port: u16,
    pub problem: Problem,
    /// SHA-256 of the server's certificate, "AB:CD:…".
    pub fingerprint: String,
}

impl fmt::Display for Untrusted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Tern can’t verify the security certificate of {}: it {}.", self.host, self.problem.words())
    }
}

/// Why a TLS connection couldn't be made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectError {
    Untrusted(Untrusted),
    /// Anything else (can't reach the server, STARTTLS refused, …), in words.
    Other(String),
}

impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConnectError::Untrusted(u) => u.fmt(f),
            ConnectError::Other(why) => f.write_str(why),
        }
    }
}

impl ConnectError {
    /// As a Tern error: a certificate problem becomes `Error::Certificate` (with the
    /// fingerprint the account trusted, `trusted`), anything else `Error::Network`.
    pub fn into_error(self, trusted: Option<&str>) -> tern_core::Error {
        match self {
            ConnectError::Untrusted(u) => tern_core::Error::Certificate(Box::new(tern_core::CertIssue {
                message: u.to_string(),
                host: u.host,
                port: u.port,
                fingerprint: u.fingerprint,
                trusted: trusted.map(str::to_owned),
            })),
            ConnectError::Other(why) => tern_core::Error::Network(why),
        }
    }
}

/// A certificate's SHA-256 fingerprint as people compare them: "AB:CD:…" (32 pairs).
pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// Whether two fingerprints are the same, however they were written (colons, spaces, case).
pub fn same_fingerprint(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.chars().filter(char::is_ascii_hexdigit).map(|c| c.to_ascii_uppercase()).collect::<String>();
    let (a, b) = (norm(a), norm(b));
    a.len() == 64 && a == b
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    // Explicit provider: other crates in the tree may enable aws-lc-rs too, which makes the
    // process-default lookup ambiguous.
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Mozilla's roots plus the system store (/etc/ssl/certs), so self-hosted servers signed by a
/// locally trusted authority work too. Loaded once.
fn roots() -> Arc<RootCertStore> {
    static ROOTS: OnceLock<Arc<RootCertStore>> = OnceLock::new();
    ROOTS
        .get_or_init(|| {
            let mut roots = RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
            let native = rustls_native_certs::load_native_certs();
            let (added, _ignored) = roots.add_parsable_certificates(native.certs);
            tracing::debug!(added, "loaded system root certificates");
            Arc::new(roots)
        })
        .clone()
}

/// The certificate problem seen during the last handshake, with the certificate's fingerprint.
type Seen = Arc<Mutex<Option<(String, Problem)>>>;

/// The usual checks, plus: a certificate whose fingerprint was trusted is accepted, and a
/// failed check remembers what failed.
#[derive(Debug)]
struct Verifier {
    inner: Arc<WebPkiServerVerifier>,
    trusted: Option<String>,
    seen: Seen,
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match self.inner.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now) {
            Ok(ok) => Ok(ok),
            Err(e) => {
                let fp = fingerprint(end_entity.as_ref());
                if self.trusted.as_deref().is_some_and(|t| same_fingerprint(t, &fp)) {
                    return Ok(ServerCertVerified::assertion());
                }
                if let Some(problem) = Problem::from_rustls(&e) {
                    *self.seen.lock().unwrap_or_else(|p| p.into_inner()) = Some((fp, problem));
                }
                Err(e)
            }
        }
    }

    // The handshake signatures are always checked: a trusted certificate is only accepted from
    // a server that holds its private key.
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

/// How TLS starts on a new connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start {
    /// TLS from the first byte (IMAP 993, SMTP 465).
    Implicit,
    /// Plain IMAP, then STARTTLS (143). The server's greeting is read first.
    ImapStartTls,
    /// Plain SMTP, then EHLO and STARTTLS (587). The server's greeting is read first.
    SmtpStartTls,
}

/// Connect to `host:port` and start TLS, accepting the certificate with fingerprint `trusted`
/// even if the system doesn't trust it. After `Implicit` the server's greeting is still
/// unread; after a STARTTLS it has been read (no new greeting follows, RFC 3501 / 3207).
pub async fn open(host: &str, port: u16, start: Start, trusted: Option<&str>) -> Result<Stream, ConnectError> {
    tokio::time::timeout(TIMEOUT, open_now(host, port, start, trusted))
        .await
        .unwrap_or_else(|_| Err(ConnectError::Other(format!("{host} didn’t answer in time"))))
}

async fn open_now(host: &str, port: u16, start: Start, trusted: Option<&str>) -> Result<Stream, ConnectError> {
    let other = |e: &dyn fmt::Display| ConnectError::Other(e.to_string());
    let name = ServerName::try_from(host.to_string()).map_err(|e| other(&format!("“{host}” isn’t a server name: {e}")))?;
    let mut tcp = TcpStream::connect((host, port)).await.map_err(|e| other(&format!("couldn’t reach {host}:{port}: {e}")))?;
    match start {
        Start::Implicit => {}
        Start::ImapStartTls => {
            read_line(&mut tcp).await?; // greeting
            tcp.write_all(b"T0 STARTTLS\r\n").await.map_err(|e| other(&e))?;
            loop {
                let line = read_line(&mut tcp).await?;
                if let Some(rest) = line.strip_prefix("T0 ") {
                    if !rest.to_ascii_uppercase().starts_with("OK") {
                        return Err(other(&format!("{host} refused STARTTLS: {}", rest.trim())));
                    }
                    break;
                }
            }
        }
        Start::SmtpStartTls => {
            smtp_reply(&mut tcp, "220").await.map_err(|why| other(&format!("{host}: {why}")))?;
            tcp.write_all(b"EHLO tern\r\n").await.map_err(|e| other(&e))?;
            smtp_reply(&mut tcp, "250").await.map_err(|why| other(&format!("{host}: {why}")))?;
            tcp.write_all(b"STARTTLS\r\n").await.map_err(|e| other(&e))?;
            smtp_reply(&mut tcp, "220").await.map_err(|why| other(&format!("{host} refused STARTTLS: {why}")))?;
        }
    }
    handshake(host, port, name, tcp, trusted).await
}

async fn handshake(
    host: &str,
    port: u16,
    name: ServerName<'static>,
    tcp: TcpStream,
    trusted: Option<&str>,
) -> Result<Stream, ConnectError> {
    let seen: Seen = Arc::default();
    let inner = WebPkiServerVerifier::builder_with_provider(roots(), provider())
        .build()
        .map_err(|e| ConnectError::Other(format!("TLS setup failed: {e}")))?;
    let verifier = Verifier { inner, trusted: trusted.map(str::to_owned), seen: seen.clone() };
    let config = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| ConnectError::Other(format!("TLS setup failed: {e}")))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    match TlsConnector::from(Arc::new(config)).connect(name, tcp).await {
        Ok(stream) => Ok(stream),
        Err(e) => match seen.lock().unwrap_or_else(|p| p.into_inner()).take() {
            Some((fingerprint, problem)) => {
                tracing::info!(host, %e, "untrusted certificate");
                Err(ConnectError::Untrusted(Untrusted { host: host.into(), port, problem, fingerprint }))
            }
            None => Err(ConnectError::Other(format!("secure connection to {host} failed: {e}"))),
        },
    }
}

/// Look at a server's certificate without signing in: `Ok` if this computer trusts it (or it
/// is the trusted one), the problem and fingerprint if not.
pub async fn probe(host: &str, port: u16, start: Start, trusted: Option<&str>) -> Result<(), ConnectError> {
    let mut stream = open(host, port, start, trusted).await?;
    let _ = stream.shutdown().await;
    Ok(())
}

/// One line from a plain connection, byte by byte so nothing past it is consumed (the TLS
/// handshake has to start right after).
async fn read_line(tcp: &mut TcpStream) -> Result<String, ConnectError> {
    let mut line = Vec::new();
    loop {
        let mut b = [0u8; 1];
        let n = tcp.read(&mut b).await.map_err(|e| ConnectError::Other(e.to_string()))?;
        if n == 0 {
            return Err(ConnectError::Other("the server closed the connection".into()));
        }
        line.push(b[0]);
        if b[0] == b'\n' || line.len() > 8192 {
            return Ok(String::from_utf8_lossy(&line).into_owned());
        }
    }
}

/// An SMTP reply (all its lines, "250-…" up to "250 …"); `Err` with its text unless its code
/// is `code`.
async fn smtp_reply(tcp: &mut TcpStream, code: &str) -> Result<(), String> {
    loop {
        let line = read_line(tcp).await.map_err(|e| e.to_string())?;
        if line.len() < 4 || line.as_bytes()[3] != b'-' {
            return if line.starts_with(code) { Ok(()) } else { Err(line.trim().to_string()) };
        }
    }
}

#[cfg(test)]
mod tests;
