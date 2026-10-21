//! Asks real providers for their message size limit (EHLO only; no sign-in, nothing sent).
//! Run with: cargo test -p tern-smtp --test live -- --ignored --nocapture

use tern_core::account::{Security, ServerConfig};

async fn probe(host: &str, port: u16, security: Security) -> Option<u64> {
    let server = ServerConfig { host: host.into(), port, security };
    let limit = tern_smtp::size_limit(&server).await.expect("EHLO probe");
    println!("{host}:{port} SIZE = {limit:?}");
    limit
}

#[tokio::test]
#[ignore = "talks to smtp.gmail.com and smtp.mail.me.com"]
async fn gmail_and_icloud_limits() {
    assert!(probe("smtp.gmail.com", 465, Security::Tls).await.is_some_and(|n| n > 20_000_000));
    assert!(probe("smtp.mail.me.com", 587, Security::StartTls).await.is_some());
}
