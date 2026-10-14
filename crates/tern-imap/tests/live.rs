//! Live test against a real IMAP server. Ignored by default.
//!
//! Use a throwaway account — it appends, moves and expunges mail:
//!
//! ```sh
//! TERN_IMAP_HOST=mail.test TERN_IMAP_USER=tom TERN_IMAP_PASS=... \
//!   cargo test -p tern-imap --test live -- --ignored --nocapture
//! ```
//! Optional: `TERN_IMAP_PORT` (993), `TERN_IMAP_STARTTLS_PORT` (143, `0` to skip).

use std::time::Duration;

use tern_auth::Credential;
use tern_core::account::{Security, ServerConfig};
use tern_core::backend::{MailBackend, PushEvent};
use tern_core::model::{FlagColor, Flags, MailboxRole};
use tern_imap::ImapBackend;

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

fn backend(port: u16, security: Security) -> ImapBackend {
    let server = ServerConfig { host: env("TERN_IMAP_HOST").unwrap(), port, security };
    let cred = Credential::Password {
        username: env("TERN_IMAP_USER").unwrap(),
        password: env("TERN_IMAP_PASS").unwrap(),
    };
    ImapBackend::new(server, cred)
}

fn message(n: u32, subject: &str) -> Vec<u8> {
    format!(
        "From: Ann <ann@example.com>\r\nTo: Tom <tom@example.com>\r\nSubject: {subject}\r\n\
         Date: Wed, 14 Oct 2026 10:0{n}:00 -0600\r\nMessage-ID: <live-{n}-{}@tern>\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\nBody number {n}, with café.\r\n",
        std::process::id()
    )
    .into_bytes()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live IMAP server (see file docs)"]
async fn full_cycle() {
    let port = env("TERN_IMAP_PORT").and_then(|p| p.parse().ok()).unwrap_or(993);
    let b = backend(port, Security::Tls);
    b.verify().await.expect("login");

    let boxes = b.list_mailboxes().await.unwrap();
    let inbox = boxes.iter().find(|m| m.role == MailboxRole::Inbox).expect("inbox").remote_id.clone();
    let trash = boxes.iter().find(|m| m.role == MailboxRole::Trash).expect("trash").remote_id.clone();
    println!("mailboxes: {:?}", boxes.iter().map(|m| (&m.remote_id, m.role)).collect::<Vec<_>>());

    // Start clean.
    for mb in [&inbox, &trash] {
        let d = b.sync_mailbox(mb, None, None).await.unwrap();
        let ids: Vec<String> = d.added.iter().map(|e| e.remote_id.clone()).collect();
        if !ids.is_empty() {
            b.set_flags(mb, &ids, &Flags { deleted: true, ..Default::default() }).await.unwrap();
            b.expunge(mb).await.unwrap();
        }
    }

    // Append three; one already read.
    for n in 1..=3 {
        let flags = Flags { seen: n == 1, ..Default::default() };
        b.append(&inbox, &message(n, &format!("Live {n}")), &flags).await.unwrap();
    }

    // Initial sync.
    let d1 = b.sync_mailbox(&inbox, None, Some(1000)).await.unwrap();
    assert!(d1.full_resync);
    assert_eq!(d1.added.len(), 3);
    let first = &d1.added[0];
    assert_eq!(first.subject, "Live 1");
    assert!(first.flags.seen);
    assert_eq!(first.from[0].display(), "Ann");
    assert!(first.preview.as_deref().unwrap().contains("café"), "preview: {:?}", first.preview);
    let uids: Vec<String> = d1.added.iter().map(|e| e.remote_id.clone()).collect();

    // Initial sync honours the limit (newest N).
    let limited = b.sync_mailbox(&inbox, None, Some(2)).await.unwrap();
    assert_eq!(limited.added.len(), 2);
    assert_eq!(limited.added[0].subject, "Live 2");

    // Apple-style coloured flag round-trips through keywords.
    let blue = Flags { seen: true, flag: Some(FlagColor::Blue), ..Default::default() };
    b.set_flags(&inbox, &uids[..1], &blue).await.unwrap();
    let d2 = b.sync_mailbox(&inbox, Some(&d1.new_state), None).await.unwrap();
    assert!(!d2.full_resync);
    assert!(d2.added.is_empty());
    let f = d2.flags_changed.iter().find(|(u, _)| *u == uids[0]).map(|(_, f)| f.clone()).unwrap();
    assert_eq!(f.flag, Some(FlagColor::Blue));
    assert_eq!(d2.present.as_ref().unwrap().len(), 3);

    // Move to Trash: gone from INBOX's present set, appears in Trash.
    b.move_messages(&inbox, &uids[1..2], &trash).await.unwrap();
    let d3 = b.sync_mailbox(&inbox, Some(&d2.new_state), None).await.unwrap();
    assert!(!d3.present.as_ref().unwrap().contains(&uids[1]));
    let t1 = b.sync_mailbox(&trash, None, None).await.unwrap();
    assert_eq!(t1.added.len(), 1);
    assert_eq!(t1.added[0].subject, "Live 2");

    // Full body.
    let body = b.fetch_body(&inbox, &uids[2]).await.unwrap();
    assert!(body.text.unwrap().contains("Body number 3"));
    assert!(!body.raw.is_empty());

    // New mail arrives → picked up incrementally.
    b.append(&inbox, &message(4, "Live 4"), &Flags::default()).await.unwrap();
    let d4 = b.sync_mailbox(&inbox, Some(&d3.new_state), None).await.unwrap();
    assert_eq!(d4.added.len(), 1);
    assert_eq!(d4.added[0].subject, "Live 4");

    // IDLE push: a second connection appends while the first is idling.
    let watcher = std::sync::Arc::new(backend(port, Security::Tls));
    let w = watcher.clone();
    let inbox2 = inbox.clone();
    let idle = tokio::spawn(async move { w.wait_for_push(&inbox2, Duration::from_secs(20)).await });
    tokio::time::sleep(Duration::from_millis(1500)).await;
    b.append(&inbox, &message(5, "Live 5"), &Flags::default()).await.unwrap();
    let pushed = tokio::time::timeout(Duration::from_secs(15), idle).await.expect("idle returned").unwrap().unwrap();
    assert!(matches!(pushed, Some(PushEvent::MailboxChanged { .. })), "got {pushed:?}");

    // Delete from Trash permanently.
    let tid: Vec<String> = t1.added.iter().map(|e| e.remote_id.clone()).collect();
    b.set_flags(&trash, &tid, &Flags { deleted: true, seen: true, ..Default::default() }).await.unwrap();
    b.expunge(&trash).await.unwrap();
    let t2 = b.sync_mailbox(&trash, Some(&t1.new_state), None).await.unwrap();
    assert!(t2.present.unwrap().is_empty());

    // STARTTLS on the submission-style port.
    let st_port: u16 = env("TERN_IMAP_STARTTLS_PORT").and_then(|p| p.parse().ok()).unwrap_or(143);
    if st_port != 0 {
        backend(st_port, Security::StartTls).verify().await.expect("STARTTLS login");
    }

    // Wrong password is an auth error, not a hang.
    let bad = ImapBackend::new(
        ServerConfig { host: env("TERN_IMAP_HOST").unwrap(), port, security: Security::Tls },
        Credential::Password { username: env("TERN_IMAP_USER").unwrap(), password: "wrong".into() },
    );
    assert!(matches!(bad.verify().await, Err(tern_core::Error::Auth(_))));
}
