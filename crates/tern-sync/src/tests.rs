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
    /// Make mutating calls (flags, move, expunge, submit) fail as if offline.
    offline: Mutex<bool>,
    /// Reject moves into this mailbox, as a server would for a deleted folder.
    reject_moves_to: Mutex<Option<String>>,
    /// Report the next sync as a UIDVALIDITY change.
    renumber_next: Mutex<bool>,
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
        let renumbered = std::mem::take(&mut *self.renumber_next.lock().unwrap());
        Ok(MailboxDelta {
            full_resync: since.is_none() || renumbered,
            ids_invalidated: renumbered,
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
        self.check_online()?;
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
        self.check_online()?;
        if self.reject_moves_to.lock().unwrap().as_deref() == Some(to) {
            return Err(Error::Protocol(format!("NO [TRYCREATE] {to} does not exist")));
        }
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
        self.check_online()?;
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
        self.check_online()?;
        self.log.lock().unwrap().push(format!("submit {from} {rcpts:?}"));
        Ok(())
    }
}

impl Fake {
    fn check_online(&self) -> Result<()> {
        if *self.offline.lock().unwrap() {
            return Err(Error::Network("offline".into()));
        }
        Ok(())
    }

    fn server_flags(&self, mb: &str, rid: &str) -> Flags {
        let boxes = self.boxes.lock().unwrap();
        boxes[mb].iter().find(|e| e.remote_id == rid).unwrap().flags.clone()
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

fn errors(rx: &async_channel::Receiver<Event>) -> Vec<String> {
    std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|e| match e {
            Event::Error { message, .. } => Some(message),
            _ => None,
        })
        .collect()
}

fn msg_id(e: &Engine, id: AccountId, rid: &str) -> MessageId {
    let rows = e.store().list(&ListScope::Mailbox(inbox(e, id)), false, 50).unwrap();
    rows.iter().find(|m| m.envelope.remote_id == rid).unwrap().id
}

#[test]
fn ops_round_trip_as_json() {
    let ops = [
        Op::SetFlags {
            mailbox: MailboxId(1),
            mailbox_remote: "INBOX".into(),
            items: vec![("7".into(), Flags { seen: true, ..Default::default() })],
        },
        Op::Move {
            from: MailboxId(1),
            from_remote: "INBOX".into(),
            to: MailboxId(2),
            to_remote: "Archive".into(),
            remote_ids: vec!["7".into()],
        },
        Op::Destroy { mailbox: MailboxId(3), mailbox_remote: "Trash".into(), remote_ids: vec!["9".into()] },
    ];
    for op in ops {
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(serde_json::from_str::<Op>(&json).unwrap(), op);
    }
}

#[tokio::test]
async fn offline_change_is_kept_and_delivered_on_reconnect() {
    let (mut engine, fake, rx, id) = setup().await;
    while rx.try_recv().is_ok() {}
    *fake.offline.lock().unwrap() = true;
    let m = msg_id(&engine, id, "1");
    engine.handle(Command::SetFlags(vec![m], FlagChange::Seen(true)));
    engine.settle().await;
    assert!(engine.store().message(m).unwrap().envelope.flags.seen, "stays read locally");
    assert_eq!(engine.store().pending_ops(id).unwrap().len(), 1, "kept in the queue");
    assert!(errors(&rx).is_empty(), "no toast while offline");
    assert!(!fake.server_flags("INBOX", "1").seen);

    *fake.offline.lock().unwrap() = false;
    engine.handle(Command::SyncAll);
    engine.settle().await;
    assert!(fake.server_flags("INBOX", "1").seen, "delivered on reconnect");
    assert!(engine.store().pending_ops(id).unwrap().is_empty());
}

#[tokio::test]
async fn rejected_op_rolls_back_and_later_ops_still_go_out() {
    let (mut engine, fake, rx, id) = setup().await;
    *fake.offline.lock().unwrap() = true;
    let (m1, m2) = (msg_id(&engine, id, "1"), msg_id(&engine, id, "2"));
    engine.handle(Command::Move(vec![m1], MoveTarget::Trash));
    engine.handle(Command::SetFlags(vec![m2], FlagChange::Flag(Some(FlagColor::Red))));
    engine.settle().await;
    while rx.try_recv().is_ok() {}

    // Back online, but the server now refuses moves into Trash.
    *fake.offline.lock().unwrap() = false;
    *fake.reject_moves_to.lock().unwrap() = Some("Trash".into());
    engine.handle(Command::SyncAll);
    engine.settle().await;
    let errs = errors(&rx);
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].contains("Trash"));
    let rows = engine.store().list(&ListScope::Mailbox(inbox(&engine, id)), false, 50).unwrap();
    assert!(rows.iter().any(|r| r.envelope.remote_id == "1"), "moved message is back");
    assert_eq!(fake.server_flags("INBOX", "2").flag, Some(FlagColor::Red), "later op delivered");
    assert!(engine.store().pending_ops(id).unwrap().is_empty());
}

#[tokio::test]
async fn queued_ops_reach_the_server_in_order() {
    let (mut engine, fake, _rx, id) = setup().await;
    *fake.offline.lock().unwrap() = true;
    let (m1, m2) = (msg_id(&engine, id, "1"), msg_id(&engine, id, "2"));
    engine.handle(Command::SetFlags(vec![m1], FlagChange::Flag(Some(FlagColor::Blue))));
    engine.handle(Command::Move(vec![m1], MoveTarget::Trash));
    engine.handle(Command::SetFlags(vec![m2], FlagChange::Seen(true)));
    engine.handle(Command::SetFlags(vec![m2], FlagChange::Seen(false)));
    engine.settle().await;
    fake.log.lock().unwrap().clear();

    *fake.offline.lock().unwrap() = false;
    engine.handle(Command::SyncAll);
    engine.settle().await;
    let log = fake.log.lock().unwrap().clone();
    let pos = |needle: &str| log.iter().position(|l| l.starts_with(needle)).unwrap();
    assert!(pos("flags INBOX [\"1\"]") < pos("move INBOX->Trash"), "{log:?}");
    assert!(!fake.server_flags("INBOX", "2").seen, "read then unread ends unread");
}

#[tokio::test]
async fn sync_does_not_revert_queued_changes() {
    let (mut engine, fake, _rx, id) = setup().await;
    *fake.offline.lock().unwrap() = true;
    let (m1, m2) = (msg_id(&engine, id, "1"), msg_id(&engine, id, "2"));
    engine.handle(Command::SetFlags(vec![m2], FlagChange::Seen(false))); // server still says read
    engine.handle(Command::Move(vec![m1], MoveTarget::Trash));
    engine.settle().await;

    // Incremental sync: the server reports message 2 as read.
    let inbox_id = inbox(&engine, id);
    engine.handle(Command::SyncMailbox(inbox_id));
    engine.settle().await;
    let rows = engine.store().list(&ListScope::Mailbox(inbox_id), false, 50).unwrap();
    assert!(!rows.iter().find(|r| r.envelope.remote_id == "2").unwrap().envelope.flags.seen, "still unread");

    // Full re-download: message 1 is still on the server's INBOX.
    engine.store_resync(inbox_id);
    engine.settle().await;
    let rows = engine.store().list(&ListScope::Mailbox(inbox_id), false, 50).unwrap();
    assert!(!rows.iter().any(|r| r.envelope.remote_id == "1"), "queued move stays hidden");
    assert!(!rows.iter().find(|r| r.envelope.remote_id == "2").unwrap().envelope.flags.seen);
}

#[tokio::test]
async fn renumbered_mailbox_discards_its_queued_ops() {
    let (mut engine, fake, rx, id) = setup().await;
    *fake.offline.lock().unwrap() = true;
    engine.handle(Command::SetFlags(vec![msg_id(&engine, id, "1")], FlagChange::Flag(Some(FlagColor::Red))));
    engine.settle().await;
    while rx.try_recv().is_ok() {}

    *fake.renumber_next.lock().unwrap() = true;
    engine.handle(Command::SyncMailbox(inbox(&engine, id)));
    engine.settle().await;
    assert!(engine.store().pending_ops(id).unwrap().is_empty());
    let errs = errors(&rx);
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].contains("renumbered"));

    *fake.offline.lock().unwrap() = false;
    engine.handle(Command::SyncAll);
    engine.settle().await;
    assert_eq!(fake.server_flags("INBOX", "1").flag, None, "never applied to the renumbered mailbox");
}

#[tokio::test]
async fn removing_an_account_discards_its_queue() {
    let (mut engine, fake, _rx, id) = setup().await;
    *fake.offline.lock().unwrap() = true;
    engine.handle(Command::SetFlags(vec![msg_id(&engine, id, "1")], FlagChange::Seen(true)));
    engine.settle().await;
    engine.handle(Command::RemoveAccount(id));
    assert!(engine.store().pending_ops(id).unwrap().is_empty());
    *fake.offline.lock().unwrap() = false;
    engine.settle().await;
    assert!(!fake.server_flags("INBOX", "1").seen);
}

#[tokio::test]
async fn queued_ops_survive_a_restart() {
    let path = std::env::temp_dir().join(format!("tern-restart-{}.db", uuid::Uuid::new_v4()));
    let fake = Arc::new(Fake::default());
    fake.boxes.lock().unwrap().insert("INBOX".into(), vec![env(1, "Hello", false)]);
    let cfg = AccountConfig::preset(Provider::Fastmail, "Tom", "tom@x.com");
    let id = cfg.id;
    {
        let (tx, _rx) = async_channel::unbounded();
        let mut engine = Engine::new(Store::open(&path).unwrap(), tx);
        engine.add(cfg.clone(), fake.clone());
        engine.start_account(id);
        engine.settle().await;
        *fake.offline.lock().unwrap() = true;
        engine.handle(Command::SetFlags(vec![msg_id(&engine, id, "1")], FlagChange::Seen(true)));
        engine.settle().await;
    } // Tern quits with the change still queued.

    *fake.offline.lock().unwrap() = false;
    let (tx, _rx) = async_channel::unbounded();
    let mut engine = Engine::new(Store::open(&path).unwrap(), tx);
    engine.add(cfg, fake.clone());
    engine.start_account(id);
    engine.settle().await;
    assert!(fake.server_flags("INBOX", "1").seen, "delivered after restart");
    assert!(engine.store().pending_ops(id).unwrap().is_empty());
    drop(engine);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}
