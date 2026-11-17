use super::*;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};

/// A self-signed certificate for "localhost" (OpenSSL's default: it says it's an authority).
const CERT: &[u8] = include_bytes!("testdata/cert.der");
const KEY: &[u8] = include_bytes!("testdata/key.der");

/// A one-connection TLS server on 127.0.0.1. `plain` runs on the TCP connection first (for
/// STARTTLS); after the handshake it sends `greeting`.
async fn server(
    plain: impl FnOnce(TcpStream) -> std::pin::Pin<Box<dyn Future<Output = TcpStream> + Send>> + Send + 'static,
    greeting: &'static [u8],
) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = rustls::ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(CERT.to_vec())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(KEY.to_vec())),
        )
        .unwrap();
    tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let tcp = plain(tcp).await;
        if let Ok(mut tls) = TlsAcceptor::from(Arc::new(config)).accept(tcp).await {
            let _ = tls.write_all(greeting).await;
            let _ = tls.flush().await;
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    });
    port
}

fn none(tcp: TcpStream) -> std::pin::Pin<Box<dyn Future<Output = TcpStream> + Send>> {
    Box::pin(async move { tcp })
}

async fn greeting(mut s: Stream) -> String {
    let mut line = String::new();
    BufReader::new(&mut s).read_line(&mut line).await.unwrap();
    line
}

#[tokio::test]
async fn untrusted_certificate_is_refused_and_described() {
    let port = server(none, b"* OK\r\n").await;
    let err = open("localhost", port, Start::Implicit, None).await.unwrap_err();
    let ConnectError::Untrusted(u) = err else { panic!("{err:?}") };
    assert_eq!(u.problem, Problem::SelfSigned);
    assert_eq!(u.fingerprint, fingerprint(CERT));
    assert_eq!((u.host.as_str(), u.port), ("localhost", port));
    assert_eq!(u.to_string(), "Tern can’t verify the security certificate of localhost: it is self-signed.");
}

#[tokio::test]
async fn the_trusted_certificate_is_accepted() {
    let port = server(none, b"* OK hello\r\n").await;
    // However the fingerprint was written down.
    let written = fingerprint(CERT).replace(':', "").to_lowercase();
    let s = open("localhost", port, Start::Implicit, Some(&written)).await.unwrap();
    assert_eq!(greeting(s).await, "* OK hello\r\n");
}

#[tokio::test]
async fn a_different_trusted_certificate_doesnt_count() {
    let port = server(none, b"* OK\r\n").await;
    let other = "00:".repeat(31) + "00";
    let err = open("localhost", port, Start::Implicit, Some(&other)).await.unwrap_err();
    assert!(matches!(err, ConnectError::Untrusted(_)), "{err:?}");
}

#[tokio::test]
async fn smtp_starttls_then_trusted_certificate() {
    fn smtp(mut tcp: TcpStream) -> std::pin::Pin<Box<dyn Future<Output = TcpStream> + Send>> {
        Box::pin(async move {
            tcp.write_all(b"220 test ESMTP\r\n").await.unwrap();
            let mut r = BufReader::new(&mut tcp);
            let mut line = String::new();
            r.read_line(&mut line).await.unwrap();
            assert!(line.starts_with("EHLO"));
            tcp.write_all(b"250-test\r\n250-SIZE 1000\r\n250 STARTTLS\r\n").await.unwrap();
            let mut r = BufReader::new(&mut tcp);
            line.clear();
            r.read_line(&mut line).await.unwrap();
            assert_eq!(line, "STARTTLS\r\n");
            tcp.write_all(b"220 go ahead\r\n").await.unwrap();
            tcp
        })
    }
    let port = server(smtp, b"250 after tls\r\n").await;
    let fp = fingerprint(CERT);
    let s = open("localhost", port, Start::SmtpStartTls, Some(&fp)).await.unwrap();
    assert_eq!(greeting(s).await, "250 after tls\r\n");
}

#[tokio::test]
async fn unreachable_server_is_not_a_certificate_problem() {
    let port = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap().port();
    let err = probe("127.0.0.1", port, Start::Implicit, None).await.unwrap_err();
    assert!(matches!(&err, ConnectError::Other(why) if why.starts_with("couldn’t reach 127.0.0.1")), "{err:?}");
}

#[test]
fn fingerprints() {
    let fp = fingerprint(b"x");
    assert_eq!(fp.len(), 32 * 3 - 1);
    assert!(same_fingerprint(&fp, &fp.replace(':', " ").to_lowercase()));
    assert!(!same_fingerprint(&fp, &fingerprint(b"y")));
    assert!(!same_fingerprint("", ""), "an empty pin trusts nothing");
}

#[test]
fn problems_in_plain_words() {
    let u = |problem| Untrusted { host: "mail.example.org".into(), port: 993, problem, fingerprint: String::new() };
    assert!(u(Problem::Expired).to_string().ends_with("mail.example.org: it has expired."));
    assert!(u(Problem::WrongHost).to_string().contains("different server name"));
    assert!(u(Problem::UnknownIssuer).to_string().contains("authority this computer trusts"));
    let e = rustls::Error::InvalidCertificate(CertificateError::NotValidForName);
    assert_eq!(Problem::from_rustls(&e), Some(Problem::WrongHost));
    assert_eq!(Problem::from_rustls(&rustls::Error::HandshakeNotComplete), None);
}
