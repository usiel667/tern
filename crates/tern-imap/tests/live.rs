//! Live test against a real IMAP server. Ignored by default.
//!
//! Use a throwaway account — it EMPTIES the INBOX and Trash, then appends, moves and
//! expunges mail. `TERN_LIVE_WIPE_OK` must repeat the user name to confirm:
//!
//! ```sh
//! dev/live-test.sh .env.gmail-test        # a real provider, settings in a git-ignored file
//! eval "$(dev/test-imap-server.sh env)"   # or the local Dovecot, then:
//! cargo test -p tern-imap --test live -- --ignored --nocapture
//! ```
//! Variables: `TERN_IMAP_HOST`, `TERN_IMAP_USER`, `TERN_IMAP_PASS`, `TERN_LIVE_WIPE_OK`;
//! optional `TERN_IMAP_PORT` (993), `TERN_IMAP_STARTTLS_PORT` (143, `0` to skip).

use std::time::Duration;

use tern_auth::Credential;
use tern_core::account::{Security, ServerConfig};
use tern_core::backend::{MailBackend, OlderFrom, PushEvent};
use tern_core::model::{FlagColor, Flags, MailboxRole};
use tern_imap::{Extensions, ImapBackend};

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

fn backend(port: u16, security: Security) -> ImapBackend {
    let user = env("TERN_IMAP_USER").expect("TERN_IMAP_USER");
    // These tests delete mail; never run them against an account nobody said was disposable.
    assert_eq!(
        env("TERN_LIVE_WIPE_OK").as_deref(),
        Some(user.as_str()),
        "TERN_LIVE_WIPE_OK must repeat TERN_IMAP_USER: the tests empty INBOX and Trash"
    );
    let server = ServerConfig { host: env("TERN_IMAP_HOST").unwrap(), port, security };
    let cred = Credential::Password {
        username: user,
        // Google shows app passwords in groups of four; it signs in without the spaces.
        password: env("TERN_IMAP_PASS").unwrap().split_whitespace().collect(),
    };
    ImapBackend::new(server, cred)
}

/// Created by `change_tracking`; left in place for the next run.
const SCRATCH: &str = "Tern Live Test";

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

    // Scrolling back: the one message below that window, and nothing more.
    let older = b.fetch_older(&inbox, &limited.new_state, &OlderFrom::default(), 10).await.unwrap();
    assert_eq!(older.delta.added.len(), 1);
    assert_eq!(older.delta.added[0].subject, "Live 1");
    assert!(!older.more);
    let none = b.fetch_older(&inbox, &older.delta.new_state, &OlderFrom::default(), 10).await.unwrap();
    assert!(none.delta.added.is_empty() && !none.more);

    // Apple-style coloured flag round-trips through keywords.
    let blue = Flags { seen: true, flag: Some(FlagColor::Blue), ..Default::default() };
    b.set_flags(&inbox, &uids[..1], &blue).await.unwrap();
    let d2 = b.sync_mailbox(&inbox, Some(&d1.new_state), None).await.unwrap();
    assert!(!d2.full_resync);
    assert!(d2.added.is_empty());
    let f = d2.flags_changed.iter().find(|(u, _)| *u == uids[0]).map(|(_, f)| f.clone()).unwrap();
    assert_eq!(f.flag, Some(FlagColor::Blue));
    // With QRESYNC deletions come as `removed` instead of a full `present` list.
    if let Some(present) = &d2.present {
        assert_eq!(present.len(), 3);
    }

    // Move to Trash: gone from INBOX's present set, appears in Trash.
    b.move_messages(&inbox, &uids[1..2], &trash).await.unwrap();
    let d3 = b.sync_mailbox(&inbox, Some(&d2.new_state), None).await.unwrap();
    assert!(d3.removed.contains(&uids[1]) || d3.present.as_ref().is_some_and(|p| !p.contains(&uids[1])));
    let t1 = b.sync_mailbox(&trash, None, None).await.unwrap();
    assert_eq!(t1.added.len(), 1);
    assert_eq!(t1.added[0].subject, "Live 2");

    // Full body.
    let body = b.fetch_body(&inbox, &uids[2]).await.unwrap();
    assert!(body.text.unwrap().contains("Body number 3"));
    assert!(!body.raw.is_empty());

    // New mail arrives → picked up incrementally.
    let appended = b.append(&inbox, &message(4, "Live 4"), &Flags::default()).await.unwrap();
    let d4 = b.sync_mailbox(&inbox, Some(&d3.new_state), None).await.unwrap();
    assert_eq!(d4.added.len(), 1);
    assert_eq!(d4.added[0].subject, "Live 4");
    assert_eq!(appended.as_deref(), Some(d4.added[0].remote_id.as_str()), "append reports the new UID");

    // IDLE push: a second connection appends while the first is idling.
    let watcher = std::sync::Arc::new(backend(port, Security::Tls));
    let w = watcher.clone();
    let inbox2 = inbox.clone();
    // Sign in first (it keeps the session): over the internet that can take longer than the
    // pause below, and mail appended before IDLE starts isn't news to it.
    watcher.wait_for_push(&inbox, Duration::from_secs(1)).await.unwrap();
    let idle = tokio::spawn(async move { w.wait_for_push(&inbox2, Duration::from_secs(150)).await });
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let appended_at = std::time::Instant::now();
    b.append(&inbox, &message(5, "Live 5"), &Flags::default()).await.unwrap();
    // Dovecot pushes at once; Gmail took 35 s and over 45 s for an APPENDed message.
    let pushed = tokio::time::timeout(Duration::from_secs(120), idle).await.expect("idle returned").unwrap().unwrap();
    println!("IDLE push after {:?}", appended_at.elapsed());
    assert!(matches!(pushed, Some(PushEvent::MailboxChanged { .. })), "got {pushed:?}");

    // Delete from Trash permanently.
    let tid: Vec<String> = t1.added.iter().map(|e| e.remote_id.clone()).collect();
    b.set_flags(&trash, &tid, &Flags { deleted: true, seen: true, ..Default::default() }).await.unwrap();
    b.expunge(&trash).await.unwrap();
    let t2 = b.sync_mailbox(&trash, Some(&t1.new_state), None).await.unwrap();
    assert!(tid.iter().all(|u| t2.removed.contains(u) || t2.present.as_ref().is_some_and(|p| !p.contains(u))));

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

/// Change tracking (CONDSTORE / QRESYNC), run three times: with everything the
/// server offers, as if it only had CONDSTORE, and as if it had neither. Uses its own
/// scratch mailbox so it doesn't collide with `full_cycle` (on Gmail a label: deleting from
/// All Mail with EXPUNGE doesn't delete, but removing a message from a label does). A second connection plays
/// "another device".
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live IMAP server (see file docs)"]
async fn change_tracking() {
    let port = env("TERN_IMAP_PORT").and_then(|p| p.parse().ok()).unwrap_or(993);
    let condstore_only = Extensions { condstore: true, qresync: false };
    for (name, ext) in [("qresync", Extensions::ALL), ("condstore", condstore_only), ("neither", Extensions::NONE)] {
        println!("--- {name}");
        let b = backend(port, Security::Tls);
        b.limit_extensions(ext).await;
        let other = backend(port, Security::Tls);
        let boxes = other.list_mailboxes().await.unwrap();
        let archive = SCRATCH.to_string();
        other.create_mailbox(&archive).await.expect("create scratch mailbox");

        // Start clean, then three messages.
        let d = other.sync_mailbox(&archive, None, None).await.unwrap();
        let ids: Vec<String> = d.added.iter().map(|e| e.remote_id.clone()).collect();
        if !ids.is_empty() {
            other.set_flags(&archive, &ids, &Flags { deleted: true, ..Default::default() }).await.unwrap();
            other.expunge(&archive).await.unwrap();
        }
        for n in 1..=3 {
            other.append(&archive, &message(n, &format!("Tracked {n}")), &Flags::default()).await.unwrap();
        }
        let d1 = b.sync_mailbox(&archive, None, Some(1000)).await.unwrap();
        assert_eq!(d1.added.len(), 3);
        // What the server really offers within the limit (Gmail has CONDSTORE, not QRESYNC).
        let used = b.extensions().await;
        let name = if used.qresync { "qresync" } else if used.condstore { "condstore" } else { "neither" };
        println!("    using {name}");
        let uids: Vec<String> = d1.added.iter().map(|e| e.remote_id.clone()).collect();

        // Idle mailbox.
        let d2 = b.sync_mailbox(&archive, Some(&d1.new_state), None).await.unwrap();
        assert!(d2.added.is_empty() && d2.removed.is_empty() && !d2.full_resync);
        match name {
            "qresync" => assert!(d2.flags_changed.is_empty() && d2.present.is_none(), "{d2:?}"),
            "condstore" => assert!(d2.flags_changed.is_empty() && d2.present.as_ref().unwrap().len() == 3),
            _ => assert_eq!(d2.flags_changed.len(), 3),
        }

        // One message flagged elsewhere: only it comes back (all three without tracking).
        let red = Flags { seen: true, flag: Some(FlagColor::Red), ..Default::default() };
        other.set_flags(&archive, &uids[..1], &red).await.unwrap();
        let d3 = b.sync_mailbox(&archive, Some(&d2.new_state), None).await.unwrap();
        let expect = if name == "neither" { 3 } else { 1 };
        assert_eq!(d3.flags_changed.len(), expect, "{d3:?}");
        let f = d3.flags_changed.iter().find(|(u, _)| *u == uids[0]).unwrap();
        assert_eq!(f.1.flag, Some(FlagColor::Red));

        // Deleted elsewhere.
        let gone = |d: &tern_core::backend::MailboxDelta, uid: &String| {
            d.removed.contains(uid) || d.present.as_ref().is_some_and(|p| !p.contains(uid))
        };
        other.set_flags(&archive, &uids[1..2], &Flags { deleted: true, ..Default::default() }).await.unwrap();
        other.expunge(&archive).await.unwrap();
        let d4 = b.sync_mailbox(&archive, Some(&d3.new_state), None).await.unwrap();
        assert!(gone(&d4, &uids[1]), "{d4:?}");
        if name == "qresync" {
            assert_eq!(d4.removed, vec![uids[1].clone()]);
            assert!(d4.present.is_none());
        }

        // Long session: 240 commands across two mailboxes on one connection, then a
        // deletion elsewhere must still be seen. (Against Dovecot the client's
        // 100-slot notice buffer stays nearly empty here; draining it before each
        // sync guards servers that send more unsolicited notices.)
        let drafts = boxes.iter().find(|m| m.role == MailboxRole::Drafts).expect("drafts").remote_id.clone();
        for _ in 0..120 {
            b.sync_mailbox(&drafts, None, None).await.unwrap();
            b.fetch_body(&archive, &uids[0]).await.unwrap();
        }
        other.set_flags(&archive, &uids[2..3], &Flags { deleted: true, ..Default::default() }).await.unwrap();
        other.expunge(&archive).await.unwrap();
        let d5 = b.sync_mailbox(&archive, Some(&d4.new_state), None).await.unwrap();
        assert!(gone(&d5, &uids[2]), "deletion seen after a long session: {d5:?}");

        // A cursor from before change tracking (v1) upgrades without a re-download.
        let v2 = &d5.new_state.0;
        assert!(v2.starts_with("v2:"), "{v2}");
        let parts: Vec<&str> = v2.split(':').collect();
        let v1 = tern_core::backend::SyncState(format!("v1:{}:{}:{}", parts[1], parts[2], parts[3]));
        let d6 = b.sync_mailbox(&archive, Some(&v1), None).await.unwrap();
        assert!(!d6.full_resync && d6.added.is_empty());
        let modseq: u64 = d6.new_state.0.rsplit(':').next().unwrap().parse().unwrap();
        assert_eq!(modseq > 0, name != "neither", "{}", d6.new_state.0);
    }
}
