use super::*;
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use std::sync::Mutex;
use tern_core::account::Provider;
use tern_core::backend::SyncState;
use tern_store::ListScope;

/// In-memory server: mailboxes of envelopes, plus a log of mutating calls.
#[derive(Default)]
struct Fake {
    boxes: Mutex<HashMap<String, Vec<Envelope>>>,
    log: Mutex<Vec<String>>,
    /// Make `submit` fail as if offline.
    offline: Mutex<bool>,
    /// Make `list_mailboxes` fail as if the password were wrong.
    bad_password: Mutex<bool>,
}

fn env(uid: u32, subject: &str, seen: bool) -> Envelope {
    Envelope {
        remote_id: uid.to_string(),
        message_id: Some(format!("<{uid}@x>")),
        in_reply_to: None,
        references: vec![],
        subject: subject.into(),
        from: vec![Address::new(Some("Ann"), "ann@x.com")],
        to: vec![Address::new(None, "tom@x.com")],
        cc: vec![],
        date: Utc.with_ymd_and_hms(2026, 10, 1, uid, 0, 0).unwrap(),
        flags: Flags { seen, ..Default::default() },
        size: 1,
        has_attachments: false,
        preview: None,
    }
}

#[async_trait]
impl MailBackend for Fake {
    fn kind(&self) -> &'static str {
        "test"
    }
    async fn list_mailboxes(&self) -> Result<Vec<RemoteMailbox>> {
        self.log.lock().unwrap().push("list".into());
        if *self.bad_password.lock().unwrap() {
            return Err(Error::Auth("bad password".into()));
        }
        let mb = |id: &str, role| RemoteMailbox {
            remote_id: id.into(),
            name: id.into(),
            parent_remote_id: None,
            role,
            selectable: true,
        };
        Ok(vec![mb("INBOX", MailboxRole::Inbox), mb("Trash", MailboxRole::Trash), mb("Sent", MailboxRole::Sent)])
    }
    async fn sync_mailbox(&self, mailbox: &str, since: Option<&SyncState>, _: Option<u32>) -> Result<MailboxDelta> {
        let all = self.boxes.lock().unwrap().get(mailbox).cloned().unwrap_or_default();
        Ok(MailboxDelta {
            full_resync: since.is_none(),
            present: Some(all.iter().map(|e| e.remote_id.clone()).collect()),
            added: all,
            new_state: SyncState("s".into()),
            ..Default::default()
        })
    }
    async fn fetch_body(&self, _: &str, rid: &str) -> Result<MessageBody> {
        Ok(tern_core::mime::body_from_raw(format!("Subject: x\r\n\r\nbody of {rid}").into_bytes()))
    }
    async fn set_flags(&self, mb: &str, rids: &[String], flags: &Flags) -> Result<()> {
        self.log.lock().unwrap().push(format!("flags {mb} {rids:?} seen={}", flags.seen));
        let mut boxes = self.boxes.lock().unwrap();
        for e in boxes.get_mut(mb).into_iter().flatten() {
            if rids.contains(&e.remote_id) {
                e.flags = flags.clone();
            }
        }
        Ok(())
    }
    async fn move_messages(&self, from: &str, rids: &[String], to: &str) -> Result<Vec<String>> {
        self.log.lock().unwrap().push(format!("move {from}->{to} {rids:?}"));
        let mut boxes = self.boxes.lock().unwrap();
        let src = boxes.entry(from.into()).or_default();
        let (moved, keep): (Vec<_>, Vec<_>) = src.drain(..).partition(|e| rids.contains(&e.remote_id));
        *src = keep;
        boxes.entry(to.into()).or_default().extend(moved);
        Ok(vec![])
    }
    async fn append(&self, mb: &str, _: &[u8], _: &Flags) -> Result<Option<String>> {
        self.log.lock().unwrap().push(format!("append {mb}"));
        Ok(None)
    }
    async fn expunge(&self, mb: &str) -> Result<()> {
        self.log.lock().unwrap().push(format!("expunge {mb}"));
        Ok(())
    }
    async fn wait_for_push(&self, _: &str, _: Duration) -> Result<Option<PushEvent>> {
        Ok(None)
    }
    fn handles_submission(&self) -> bool {
        true
    }
    async fn submit(&self, raw: &[u8], from: &str, rcpts: &[String]) -> Result<()> {
        assert!(!raw.is_empty());
        if *self.offline.lock().unwrap() {
            return Err(Error::Network("offline".into()));
        }
        self.log.lock().unwrap().push(format!("submit {from} {rcpts:?}"));
        Ok(())
    }
}

async fn setup() -> (Engine, Arc<Fake>, async_channel::Receiver<Event>, AccountId) {
    let fake = Arc::new(Fake::default());
    fake.boxes.lock().unwrap().insert(
        "INBOX".into(),
        vec![env(1, "Hello", false), env(2, "Lunch?", true)],
    );
    let (tx, rx) = async_channel::unbounded();
    let mut engine = Engine::new(Store::open_in_memory().unwrap(), tx);
    let cfg = AccountConfig::preset(Provider::Fastmail, "Tom", "tom@x.com");
    let id = cfg.id;
    engine.add(cfg, fake.clone());
    engine.start_account(id);
    engine.settle().await;
    (engine, fake, rx, id)
}

fn inbox(e: &Engine, id: AccountId) -> MailboxId {
    e.store().mailbox_by_role(id, MailboxRole::Inbox).unwrap().unwrap().id
}

#[tokio::test]
async fn initial_sync_fills_store() {
    let (engine, _, rx, id) = setup().await;
    let boxes = engine.store().mailboxes(Some(id)).unwrap();
    assert_eq!(boxes.len(), 3);
    assert_eq!(boxes[0].unread, 1);
    let rows = engine.store().list(&ListScope::Role(MailboxRole::Inbox), true, 50).unwrap();
    assert_eq!(rows.len(), 2);
    let events: Vec<Event> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert!(events.iter().any(|e| matches!(e, Event::SyncFinished(_))));
    // Initial sync must not spam notifications.
    assert!(!events.iter().any(|e| matches!(e, Event::NewMail { .. })));
}

#[tokio::test]
async fn new_mail_notifies_after_initial_sync() {
    let (mut engine, fake, rx, id) = setup().await;
    while rx.try_recv().is_ok() {}
    fake.boxes.lock().unwrap().get_mut("INBOX").unwrap().push(env(3, "Urgent", false));
    engine.handle(Command::SyncMailbox(inbox(&engine, id)));
    engine.settle().await;
    let events: Vec<Event> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert!(events.iter().any(|e| matches!(e, Event::NewMail { subject, .. } if subject == "Urgent")));
}

#[tokio::test]
async fn mark_read_is_optimistic_and_pushed() {
    let (mut engine, fake, _, id) = setup().await;
    let msg = engine.store().list(&ListScope::Unread, false, 50).unwrap()[0].id;
    engine.handle(Command::SetFlags(vec![msg], FlagChange::Seen(true)));
    // Local state changes before the server round-trip completes.
    assert!(engine.store().list(&ListScope::Unread, false, 50).unwrap().is_empty());
    engine.settle().await;
    assert!(fake.log.lock().unwrap().iter().any(|l| l.starts_with("flags INBOX") && l.ends_with("seen=true")));
    assert_eq!(engine.store().mailbox(inbox(&engine, id)).unwrap().unread, 0);
}

#[tokio::test]
async fn delete_moves_to_trash_then_trash_delete_expunges() {
    let (mut engine, fake, _, id) = setup().await;
    let msg = engine.store().list(&ListScope::Mailbox(inbox(&engine, id)), false, 50).unwrap()[0].id;
    engine.handle(Command::Move(vec![msg], MoveTarget::Trash));
    engine.settle().await;
    assert!(fake.log.lock().unwrap().iter().any(|l| l.starts_with("move INBOX->Trash")));
    let trash = engine.store().list(&ListScope::Role(MailboxRole::Trash), false, 50).unwrap();
    assert_eq!(trash.len(), 1, "destination resynced");

    engine.handle(Command::Move(vec![trash[0].id], MoveTarget::Trash));
    engine.settle().await;
    assert!(fake.log.lock().unwrap().iter().any(|l| l == "expunge Trash"));
}

#[tokio::test]
async fn send_reply_submits_and_marks_answered() {
    let (mut engine, fake, rx, id) = setup().await;
    let orig = engine.store().list(&ListScope::Mailbox(inbox(&engine, id)), false, 50).unwrap()[0].clone();
    let me = Address::new(Some("Tom"), "tom@x.com");
    let draft = tern_core::compose::respond(
        tern_core::compose::ReplyKind::Reply,
        &orig.envelope,
        None,
        &me,
        &["tom@x.com".into()],
    );
    engine.handle(Command::Send {
        account: id,
        draft: Box::new(draft),
        responding_to: Some((orig.id, FlagChange::Answered)),
    });
    engine.settle().await;
    assert!(fake.log.lock().unwrap().iter().any(|l| l.contains("submit tom@x.com") && l.contains("ann@x.com")));
    assert!(engine.store().outbox().unwrap().is_empty());
    assert!(engine.store().message(orig.id).unwrap().envelope.flags.answered);
    assert!(std::iter::from_fn(|| rx.try_recv().ok()).any(|e| matches!(e, Event::Sent)));
}

#[tokio::test]
async fn body_fetch_is_cached() {
    let (mut engine, fake, rx, id) = setup().await;
    let msg = engine.store().list(&ListScope::Mailbox(inbox(&engine, id)), false, 50).unwrap()[0].id;
    engine.handle(Command::FetchBody(msg));
    engine.settle().await;
    let body = engine.store().body(msg).unwrap().unwrap();
    assert!(body.text.unwrap().contains("body of"));
    while rx.try_recv().is_ok() {}
    engine.handle(Command::FetchBody(msg));
    assert!(matches!(rx.try_recv(), Ok(Event::BodyReady(_))), "served from cache synchronously");
    drop(fake);
}

fn draft_with_bcc() -> Draft {
    Draft {
        to: vec![Address::new(None, "ann@x.com")],
        bcc: vec![Address::new(None, "secret@x.com")],
        subject: "Hi".into(),
        text: "hello".into(),
        ..Default::default()
    }
}

fn submits(fake: &Fake) -> Vec<String> {
    fake.log.lock().unwrap().iter().filter(|l| l.starts_with("submit")).cloned().collect()
}

#[tokio::test]
async fn offline_send_keeps_bcc_when_retried() {
    let (mut engine, fake, _rx, id) = setup().await;
    *fake.offline.lock().unwrap() = true;
    engine.handle(Command::Send { account: id, draft: Box::new(draft_with_bcc()), responding_to: None });
    engine.settle().await;
    let queued = engine.store().outbox().unwrap();
    assert_eq!(queued.len(), 1, "failed send stays queued");
    assert_eq!(queued[0].attempts, 1);

    *fake.offline.lock().unwrap() = false;
    engine.flush_outbox();
    engine.settle().await;
    let sent = submits(&fake);
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains("secret@x.com"), "Bcc survives the retry: {sent:?}");
    assert!(engine.store().outbox().unwrap().is_empty());
}

#[tokio::test]
async fn flush_during_send_does_not_send_twice() {
    let (mut engine, fake, _rx, id) = setup().await;
    engine.handle(Command::Send { account: id, draft: Box::new(draft_with_bcc()), responding_to: None });
    // The periodic flush fires while the send is still in flight.
    engine.flush_outbox();
    engine.settle().await;
    assert_eq!(submits(&fake).len(), 1);
}

#[test]
fn auth_retry_delay_backs_off_and_caps() {
    let secs: Vec<u64> = (1..=10).map(|n| auth_retry_delay(n).as_secs()).collect();
    assert_eq!(secs, [0, 30, 60, 120, 240, 480, 960, 1800, 1800, 1800]);
    assert_eq!(auth_retry_delay(u32::MAX), AUTH_RETRY_MAX);
}

/// Process engine messages for `d` of (paused, auto-advancing) time.
async fn run_for(engine: &mut Engine, d: Duration) {
    let _ = tokio::time::timeout(d, async {
        while let Some(m) = engine.rx.recv().await {
            engine.internal(m);
        }
    })
    .await;
}

fn count(fake: &Fake, what: &str) -> usize {
    fake.log.lock().unwrap().iter().filter(|l| *l == what).count()
}

#[tokio::test(start_paused = true)]
async fn wrong_password_backs_off_instead_of_looping() {
    let (mut engine, fake, rx, id) = setup().await;
    let f = fake.clone();
    engine.connect = Arc::new(move |_| {
        let b: Arc<dyn MailBackend> = f.clone();
        Box::pin(async move { Ok(b) })
    });
    *fake.bad_password.lock().unwrap() = true;
    fake.log.lock().unwrap().clear();
    while rx.try_recv().is_ok() {}

    engine.handle(Command::SyncAll);
    // First failure reconnects straight away (expired token case), then waits 30s.
    run_for(&mut engine, Duration::from_secs(10)).await;
    assert_eq!(count(&fake, "list"), 2);
    run_for(&mut engine, Duration::from_secs(25)).await;
    assert_eq!(count(&fake, "list"), 3);
    // An hour of a wrong password: attempts at 0, 0, 30, 90, 210, 450, 930, 1890 s.
    run_for(&mut engine, Duration::from_secs(3600 - 35)).await;
    assert_eq!(count(&fake, "list"), 8);
    let toasts = std::iter::from_fn(|| rx.try_recv().ok())
        .filter(|e| matches!(e, Event::Error { .. }))
        .count();
    assert_eq!(toasts, 8, "one error per attempt, not a flood");

    // Password fixed: the next attempt succeeds and the streak resets.
    *fake.bad_password.lock().unwrap() = false;
    run_for(&mut engine, AUTH_RETRY_MAX).await;
    assert!(!engine.auth_retry.contains_key(&id));
    assert_eq!(count(&fake, "list"), 9);
}

#[tokio::test(start_paused = true)]
async fn removed_account_is_not_resurrected_by_a_pending_reconnect() {
    let (mut engine, fake, _rx, id) = setup().await;
    let f = fake.clone();
    engine.connect = Arc::new(move |_| {
        let b: Arc<dyn MailBackend> = f.clone();
        Box::pin(async move { Ok(b) })
    });
    *fake.bad_password.lock().unwrap() = true;
    engine.handle(Command::SyncAll);
    run_for(&mut engine, Duration::from_secs(10)).await; // now waiting 30s to retry
    engine.handle(Command::RemoveAccount(id));
    run_for(&mut engine, Duration::from_secs(60)).await;
    assert!(!engine.accounts.contains_key(&id));
}
