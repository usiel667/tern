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
    /// When set, syncs only return the newest N messages (a cached window);
    /// `fetch_older` extends it. The cursor is `w:<window size>`.
    window: Mutex<Option<usize>>,
    /// Make `fetch_older` fail with a protocol error.
    older_rejects: Mutex<bool>,
    /// Make `list_mailboxes` fail as if the password were wrong.
    bad_password: Mutex<bool>,
    /// Make `submit` fail as a server refusing the message would.
    refuse_submit: Mutex<bool>,
    /// What `send_limit` reports.
    limit: Mutex<Option<u64>>,
    /// Make `append` not report the new message's id (like IMAP without a usable search).
    append_without_id: Mutex<bool>,
    next_uid: Mutex<u32>,
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
        Ok(vec![
            mb("INBOX", MailboxRole::Inbox),
            mb("Trash", MailboxRole::Trash),
            mb("Sent", MailboxRole::Sent),
            mb("Drafts", MailboxRole::Drafts),
        ])
    }
    async fn sync_mailbox(&self, mailbox: &str, since: Option<&SyncState>, _: Option<u32>) -> Result<MailboxDelta> {
        self.log.lock().unwrap().push(format!("sync {mailbox}"));
        let all = self.boxes.lock().unwrap().get(mailbox).cloned().unwrap_or_default();
        let renumbered = std::mem::take(&mut *self.renumber_next.lock().unwrap());
        let (visible, state) = match *self.window.lock().unwrap() {
            None => (all, "s".to_string()),
            Some(initial) => {
                let n = window_of(since).unwrap_or(initial).min(all.len());
                (all[all.len() - n..].to_vec(), format!("w:{n}"))
            }
        };
        Ok(MailboxDelta {
            full_resync: since.is_none() || renumbered,
            ids_invalidated: renumbered,
            present: Some(visible.iter().map(|e| e.remote_id.clone()).collect()),
            added: visible,
            new_state: SyncState(state),
            ..Default::default()
        })
    }
    async fn fetch_older(&self, mailbox: &str, since: &SyncState, _: &OlderFrom, count: u32) -> Result<OlderBatch> {
        self.log.lock().unwrap().push(format!("older {mailbox}"));
        self.check_online()?;
        if *self.older_rejects.lock().unwrap() {
            return Err(Error::Protocol("NO older mail for you".into()));
        }
        let all = self.boxes.lock().unwrap().get(mailbox).cloned().unwrap_or_default();
        let n = window_of(Some(since)).unwrap_or(all.len()).min(all.len());
        let n2 = (n + count as usize).min(all.len());
        Ok(OlderBatch {
            delta: MailboxDelta {
                added: all[all.len() - n2..all.len() - n].to_vec(),
                new_state: SyncState(format!("w:{n2}")),
                ..Default::default()
            },
            more: n2 < all.len(),
        })
    }
    async fn fetch_body(&self, mb: &str, rid: &str) -> Result<MessageBody> {
        self.log.lock().unwrap().push(format!("body {mb} {rid}"));
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
    async fn append(&self, mb: &str, raw: &[u8], flags: &Flags) -> Result<Option<String>> {
        self.check_online()?;
        self.log.lock().unwrap().push(format!("append {mb}"));
        let uid = {
            let mut n = self.next_uid.lock().unwrap();
            *n = (*n).max(1000) + 1;
            *n
        };
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4).unwrap_or(raw.len());
        let env = tern_core::mime::envelope_from_parts(uid.to_string(), &raw[..split], None, flags.clone(), raw.len() as u32, None);
        self.boxes.lock().unwrap().entry(mb.into()).or_default().push(env);
        Ok((!*self.append_without_id.lock().unwrap()).then(|| uid.to_string()))
    }
    async fn expunge(&self, mb: &str) -> Result<()> {
        self.check_online()?;
        self.log.lock().unwrap().push(format!("expunge {mb}"));
        if let Some(v) = self.boxes.lock().unwrap().get_mut(mb) {
            v.retain(|e| !e.flags.deleted);
        }
        Ok(())
    }
    async fn wait_for_push(&self, _: &str, _: Duration) -> Result<Option<PushEvent>> {
        Ok(None)
    }
    fn handles_submission(&self) -> bool {
        true
    }
    async fn send_limit(&self) -> Result<Option<u64>> {
        Ok(*self.limit.lock().unwrap())
    }
    async fn submit(&self, raw: &[u8], from: &str, rcpts: &[String]) -> Result<()> {
        assert!(!raw.is_empty());
        self.check_online()?;
        if *self.refuse_submit.lock().unwrap() {
            self.log.lock().unwrap().push("refused submit".into());
            return Err(Error::Rejected("552 message too large".into()));
        }
        self.log.lock().unwrap().push(format!("submit {from} {rcpts:?}"));
        Ok(())
    }
}

fn window_of(since: Option<&SyncState>) -> Option<usize> {
    since?.0.strip_prefix("w:")?.parse().ok()
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
    assert_eq!(boxes.len(), 4);
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
        draft_key: None,
        hold: None,
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

fn body_fetches(fake: &Fake) -> Vec<String> {
    fake.log.lock().unwrap().iter().filter(|l| l.starts_with("body")).cloned().collect()
}

#[tokio::test]
async fn inbox_bodies_download_ahead() {
    let (engine, fake, _rx, id) = setup().await;
    for m in engine.store().list(&ListScope::Mailbox(inbox(&engine, id)), false, 50).unwrap() {
        assert!(engine.store().body(m.id).unwrap().is_some(), "{} not downloaded", m.envelope.subject);
    }
    let mut fetched = body_fetches(&fake);
    fetched.sort();
    assert_eq!(fetched, ["body INBOX 1", "body INBOX 2"], "each once, newest first doesn't matter here");
}

#[tokio::test]
async fn opening_twice_downloads_once() {
    let (mut engine, fake, _rx, id) = setup().await;
    // Trash isn't downloaded ahead until it's opened.
    fake.boxes.lock().unwrap().insert("Trash".into(), vec![env(7, "Old", true)]);
    let trash = engine.store().mailbox_by_role(id, MailboxRole::Trash).unwrap().unwrap().id;
    engine.sync_mailbox(trash);
    engine.settle().await;
    assert!(!body_fetches(&fake).iter().any(|l| l.starts_with("body Trash")));
    let msg = engine.store().list(&ListScope::Mailbox(trash), false, 50).unwrap()[0].id;
    engine.handle(Command::FetchBody(msg));
    engine.handle(Command::FetchBody(msg));
    engine.settle().await;
    assert!(engine.store().body(msg).unwrap().is_some());
    assert_eq!(body_fetches(&fake).iter().filter(|l| l.starts_with("body Trash")).count(), 1);
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
    engine.handle(Command::Send { account: id, draft: Box::new(draft_with_bcc()), responding_to: None, draft_key: None, hold: None });
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
    engine.handle(Command::Send { account: id, draft: Box::new(draft_with_bcc()), responding_to: None, draft_key: None, hold: None });
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
        .filter(|e| matches!(e, Event::SignInFailed { .. }))
        .count();
    assert_eq!(toasts, 8, "one error per attempt, not a flood");

    // Password fixed: the next attempt succeeds and the streak resets.
    *fake.bad_password.lock().unwrap() = false;
    run_for(&mut engine, AUTH_RETRY_MAX).await;
    assert!(!engine.auth_retry.contains_key(&id));
    assert_eq!(count(&fake, "list"), 9);
}

#[tokio::test(start_paused = true)]
async fn signing_in_again_skips_the_back_off() {
    let (mut engine, fake, _rx, id) = setup().await;
    let f = fake.clone();
    engine.connect = Arc::new(move |_| {
        let b: Arc<dyn MailBackend> = f.clone();
        Box::pin(async move { Ok(b) })
    });
    *fake.bad_password.lock().unwrap() = true;
    engine.handle(Command::SyncAll);
    run_for(&mut engine, Duration::from_secs(60)).await; // two failures in, now waiting 60s
    let before = count(&fake, "list");

    *fake.bad_password.lock().unwrap() = false;
    let cfg = engine.accounts[&id].cfg.clone();
    engine.handle(Command::SignIn(Box::new(cfg)));
    run_for(&mut engine, Duration::from_secs(5)).await;
    assert_eq!(count(&fake, "list"), before + 1, "connected right away");
    assert!(!engine.auth_retry.get(&id).is_some_and(|r| r.failures > 0));
}

#[tokio::test(start_paused = true)]
async fn signing_in_loads_an_account_that_failed_at_startup() {
    let (mut engine, fake, _rx, _) = setup().await;
    let f = fake.clone();
    engine.connect = Arc::new(move |_| {
        let b: Arc<dyn MailBackend> = f.clone();
        Box::pin(async move { Ok(b) })
    });
    // Configured, but never loaded: its password was missing when Tern started.
    let cfg = AccountConfig::preset(Provider::Fastmail, "Ann", "ann@x.com");
    let id = cfg.id;
    engine.handle(Command::SignIn(Box::new(cfg)));
    run_for(&mut engine, Duration::from_secs(5)).await;
    assert!(engine.accounts.contains_key(&id));
    assert!(engine.store().mailbox_by_role(id, MailboxRole::Inbox).unwrap().is_some());
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

#[tokio::test]
async fn push_during_a_sync_syncs_again_after_it() {
    let (mut engine, fake, _rx, id) = setup().await;
    fake.log.lock().unwrap().clear();
    engine.handle(Command::SyncMailbox(inbox(&engine, id)));
    engine.internal(Internal::Push(id, PushEvent::MailboxChanged { remote_id: "INBOX".into() }));
    engine.settle().await;
    assert_eq!(count(&fake, "sync INBOX"), 2);
    // A push with nothing in flight syncs once.
    fake.log.lock().unwrap().clear();
    engine.internal(Internal::Push(id, PushEvent::MailboxChanged { remote_id: "INBOX".into() }));
    engine.settle().await;
    assert_eq!(count(&fake, "sync INBOX"), 1);
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

/// An inbox of 12 messages of which the server's first sync returns the newest 5.
async fn setup_history() -> (Engine, Arc<Fake>, async_channel::Receiver<Event>, AccountId) {
    let fake = Arc::new(Fake::default());
    let mail: Vec<Envelope> = (1..=12).map(|n| env(n, &format!("Mail {n}"), true)).collect();
    fake.boxes.lock().unwrap().insert("INBOX".into(), mail);
    *fake.window.lock().unwrap() = Some(5);
    let (tx, rx) = async_channel::unbounded();
    let mut engine = Engine::new(Store::open_in_memory().unwrap(), tx);
    engine.older_batch = 4;
    let cfg = AccountConfig::preset(Provider::Fastmail, "Tom", "tom@x.com");
    let id = cfg.id;
    engine.add(cfg, fake.clone());
    engine.start_account(id);
    engine.settle().await;
    (engine, fake, rx, id)
}

fn cached(e: &Engine, id: AccountId) -> u32 {
    e.store().oldest_cached(inbox(e, id)).unwrap().1
}

#[tokio::test]
async fn load_older_extends_the_cached_window() {
    let (mut engine, fake, _rx, id) = setup_history().await;
    let inbox_id = inbox(&engine, id);
    assert_eq!(cached(&engine, id), 5);
    engine.handle(Command::LoadOlder(vec![inbox_id]));
    engine.settle().await;
    assert_eq!(cached(&engine, id), 9);
    assert_eq!(engine.store().oldest_cached(inbox_id).unwrap().0.as_deref(), Some("4"));
    assert_eq!(engine.store().sync_state(inbox_id).unwrap().unwrap().0, "w:9");

    // A queued action on an older message isn't reverted by the next sync.
    *fake.offline.lock().unwrap() = true;
    engine.handle(Command::Move(vec![msg_id(&engine, id, "4")], MoveTarget::Trash));
    engine.settle().await;
    engine.handle(Command::SyncMailbox(inbox_id));
    engine.settle().await;
    let rows = engine.store().list(&ListScope::Mailbox(inbox_id), false, 50).unwrap();
    assert!(!rows.iter().any(|r| r.envelope.remote_id == "4"));
    assert_eq!(rows.len(), 8, "the other older rows survive the incremental sync");
}

#[tokio::test]
async fn load_older_waits_for_a_running_sync_and_is_not_duplicated() {
    let (mut engine, fake, _rx, id) = setup_history().await;
    let inbox_id = inbox(&engine, id);
    // Asked for while a regular sync is in flight: runs after it.
    engine.handle(Command::SyncMailbox(inbox_id));
    engine.handle(Command::LoadOlder(vec![inbox_id]));
    engine.settle().await;
    assert_eq!(count(&fake, "older INBOX"), 1);
    assert_eq!(cached(&engine, id), 9);

    // Asked for twice while loading: one download.
    engine.handle(Command::LoadOlder(vec![inbox_id]));
    engine.handle(Command::LoadOlder(vec![inbox_id]));
    engine.settle().await;
    assert_eq!(count(&fake, "older INBOX"), 2);
    assert_eq!(cached(&engine, id), 12);

    // A sync asked for during a load isn't lost, and keeps the older rows.
    fake.boxes.lock().unwrap().get_mut("INBOX").unwrap().push(env(13, "New", false));
    engine.exhausted.clear();
    engine.handle(Command::LoadOlder(vec![inbox_id]));
    engine.handle(Command::SyncMailbox(inbox_id));
    engine.settle().await;
    assert_eq!(cached(&engine, id), 13);
}

#[tokio::test]
async fn load_older_stops_at_the_start_and_handles_failures() {
    let (mut engine, fake, rx, id) = setup_history().await;
    let inbox_id = inbox(&engine, id);
    engine.older_batch = 100;
    while rx.try_recv().is_ok() {}

    // Offline: no toast, and the slot is released for a later try.
    *fake.offline.lock().unwrap() = true;
    engine.handle(Command::LoadOlder(vec![inbox_id]));
    engine.settle().await;
    assert!(errors(&rx).is_empty());
    *fake.offline.lock().unwrap() = false;

    // Refused: one toast, and no rollback re-download of the mailbox.
    *fake.older_rejects.lock().unwrap() = true;
    engine.handle(Command::LoadOlder(vec![inbox_id]));
    engine.settle().await;
    assert_eq!(errors(&rx).len(), 1);
    assert_eq!(engine.store().sync_state(inbox_id).unwrap().unwrap().0, "w:5");
    *fake.older_rejects.lock().unwrap() = false;

    // Everything older in one batch, then no more requests.
    engine.handle(Command::LoadOlder(vec![inbox_id]));
    engine.settle().await;
    assert_eq!(cached(&engine, id), 12);
    engine.handle(Command::LoadOlder(vec![inbox_id]));
    engine.settle().await;
    assert_eq!(count(&fake, "older INBOX"), 3);
}

#[tokio::test]
async fn demo_history_loads_until_exhausted() {
    let (tx, _rx) = async_channel::unbounded();
    let mut engine = Engine::new(Store::open_in_memory().unwrap(), tx);
    let cfg = crate::demo::demo_account();
    let id = cfg.id;
    engine.add(cfg, Arc::new(crate::demo::DemoBackend::new()));
    engine.start_account(id);
    engine.settle().await;
    let inbox_id = inbox(&engine, id);
    assert_eq!(cached(&engine, id), 50, "first sync is the newest window");

    let mut rounds = 0;
    while !engine.exhausted.contains(&inbox_id) {
        engine.handle(Command::LoadOlder(vec![inbox_id]));
        engine.settle().await;
        rounds += 1;
        assert!(rounds < 10);
    }
    assert_eq!(cached(&engine, id), 607, "7 recent + 600 generated");
    assert_eq!(rounds, 3, "250 per batch");

    // Archiving an old message: it shows up in Archive, not lost below its window.
    let oldest = engine.store().oldest_cached(inbox_id).unwrap().0.unwrap();
    let rows = engine.store().list(&ListScope::Mailbox(inbox_id), false, 1000).unwrap();
    let old = rows.iter().find(|m| m.envelope.remote_id == oldest).unwrap().id;
    engine.handle(Command::Move(vec![old], MoveTarget::Archive));
    engine.settle().await;
    let archive = engine.store().mailbox_by_role(id, MailboxRole::Archive).unwrap().unwrap().id;
    let rows = engine.store().list(&ListScope::Mailbox(archive), false, 50).unwrap();
    assert!(rows.iter().any(|m| m.envelope.remote_id == oldest));
}

fn events(rx: &async_channel::Receiver<Event>) -> Vec<Event> {
    std::iter::from_fn(|| rx.try_recv().ok()).collect()
}

#[tokio::test]
async fn refused_send_is_kept_and_not_retried() {
    let (mut engine, fake, rx, id) = setup().await;
    let _ = events(&rx);
    *fake.refuse_submit.lock().unwrap() = true;
    let mut draft = draft_with_bcc();
    draft.subject = "Big report".into();
    engine.handle(Command::Send { account: id, draft: Box::new(draft), responding_to: None, draft_key: None, hold: None });
    engine.settle().await;

    assert!(engine.store().outbox().unwrap().is_empty(), "no longer queued");
    let refused = engine.store().outbox_rejected().unwrap();
    assert_eq!(refused.len(), 1, "kept for the user");
    assert_eq!(refused[0].last_error.as_deref(), Some("552 message too large"));
    let evs = events(&rx);
    assert!(
        evs.iter().any(|e| matches!(e, Event::SendRejected { subject, reason, .. }
            if subject == "Big report" && reason == "552 message too large")),
        "{evs:?}"
    );
    assert!(!evs.iter().any(|e| matches!(e, Event::Error { .. })), "one notice, not a generic error too");

    engine.flush_outbox();
    engine.settle().await;
    assert_eq!(count(&fake, "refused submit"), 1, "the periodic flush doesn't resend it");

    // Try Again once the problem is fixed.
    *fake.refuse_submit.lock().unwrap() = false;
    engine.handle(Command::RetrySend(refused[0].id));
    engine.settle().await;
    assert_eq!(submits(&fake).len(), 1);
    assert!(engine.store().outbox_rejected().unwrap().is_empty());
    assert!(engine.store().outbox().unwrap().is_empty());
}

#[tokio::test]
async fn refused_send_can_be_discarded() {
    let (mut engine, fake, _rx, id) = setup().await;
    *fake.refuse_submit.lock().unwrap() = true;
    engine.handle(Command::Send { account: id, draft: Box::new(draft_with_bcc()), responding_to: None, draft_key: None, hold: None });
    engine.settle().await;
    let refused = engine.store().outbox_rejected().unwrap();
    engine.handle(Command::DiscardSend(refused[0].id));
    engine.settle().await;
    assert!(engine.store().outbox_rejected().unwrap().is_empty());
}

#[tokio::test]
async fn over_the_limit_is_refused_without_uploading() {
    let (mut engine, fake, rx, id) = setup().await;
    *fake.limit.lock().unwrap() = Some(100);
    engine.probe_send_limit(id);
    engine.settle().await;
    assert!(
        events(&rx).iter().any(|e| matches!(e, Event::SendLimit { bytes: Some(100), .. })),
        "the UI hears about the limit"
    );
    engine.handle(Command::Send { account: id, draft: Box::new(draft_with_bcc()), responding_to: None, draft_key: None, hold: None });
    engine.settle().await;
    assert!(submits(&fake).is_empty(), "never sent to the server");
    let refused = engine.store().outbox_rejected().unwrap();
    assert!(refused[0].last_error.as_deref().unwrap().contains("accepts up to"), "{refused:?}");
}

#[tokio::test]
async fn network_failure_still_retries() {
    let (mut engine, fake, _rx, id) = setup().await;
    *fake.offline.lock().unwrap() = true;
    engine.handle(Command::Send { account: id, draft: Box::new(draft_with_bcc()), responding_to: None, draft_key: None, hold: None });
    engine.settle().await;
    assert_eq!(engine.store().outbox().unwrap().len(), 1, "still queued");
    assert!(engine.store().outbox_rejected().unwrap().is_empty());
}

fn draft(subject: &str) -> Box<Draft> {
    Box::new(Draft {
        to: vec![Address::new(None, "ann@x.com")],
        bcc: vec![Address::new(None, "boss@x.com")],
        subject: subject.into(),
        text: "half-written".into(),
        ..Default::default()
    })
}

fn server_drafts(fake: &Fake) -> Vec<String> {
    fake.boxes.lock().unwrap().get("Drafts").map(|v| v.iter().map(|e| e.subject.clone()).collect()).unwrap_or_default()
}

fn cached_drafts(e: &Engine, id: AccountId) -> Vec<String> {
    let mb = e.store().mailbox_by_role(id, MailboxRole::Drafts).unwrap().unwrap().id;
    e.store().list(&tern_store::ListScope::Mailbox(mb), false, 50).unwrap().into_iter().map(|m| m.envelope.subject).collect()
}

#[tokio::test]
async fn autosave_keeps_one_server_copy_per_draft() {
    let (mut engine, fake, _rx, id) = setup().await;
    let save = |subject: &str| Command::SaveDraft { key: "k1".into(), account: id, draft: draft(subject) };
    engine.handle(save("Plan v1"));
    engine.settle().await;
    assert_eq!(server_drafts(&fake), ["Plan v1"]);
    assert_eq!(cached_drafts(&engine, id), ["Plan v1"], "shows up in the Drafts mailbox");

    engine.handle(save("Plan v2"));
    engine.settle().await;
    assert_eq!(server_drafts(&fake), ["Plan v2"], "the old copy is replaced, not duplicated");
    assert_eq!(cached_drafts(&engine, id), ["Plan v2"]);
    let row = engine.store().draft("k1").unwrap().unwrap();
    assert!(row.uploaded);
    let back = tern_core::mime::draft_from_raw(&row.raw, &[]).unwrap();
    assert_eq!(back.bcc[0].email, "boss@x.com", "drafts keep Bcc");
}

#[tokio::test]
async fn sending_or_discarding_removes_the_draft() {
    let (mut engine, fake, _rx, id) = setup().await;
    engine.handle(Command::SaveDraft { key: "a".into(), account: id, draft: draft("To send") });
    engine.handle(Command::SaveDraft { key: "b".into(), account: id, draft: draft("To drop") });
    engine.settle().await;
    assert_eq!(server_drafts(&fake).len(), 2);

    engine.handle(Command::Send { account: id, draft: draft("To send"), responding_to: None, draft_key: Some("a".into()), hold: None });
    engine.handle(Command::DiscardDraft("b".into()));
    engine.settle().await;
    assert!(server_drafts(&fake).is_empty(), "{:?}", server_drafts(&fake));
    assert!(engine.store().draft("a").unwrap().is_none() && engine.store().draft("b").unwrap().is_none());
    assert_eq!(submits(&fake).len(), 1, "the message itself was sent");
}

#[tokio::test]
async fn offline_drafts_wait_and_upload_later() {
    let (mut engine, fake, _rx, id) = setup().await;
    *fake.offline.lock().unwrap() = true;
    engine.handle(Command::SaveDraft { key: "k".into(), account: id, draft: draft("On the train") });
    engine.settle().await;
    assert!(server_drafts(&fake).is_empty());
    assert_eq!(engine.store().drafts_pending().unwrap().len(), 1, "kept on this computer");

    *fake.offline.lock().unwrap() = false;
    engine.flush_outbox();
    engine.settle().await;
    assert_eq!(server_drafts(&fake), ["On the train"]);
    assert!(engine.store().drafts_pending().unwrap().is_empty());
}

#[tokio::test]
async fn editing_a_server_draft_replaces_it() {
    let (mut engine, fake, _rx, id) = setup().await;
    // A draft written on the phone.
    fake.boxes.lock().unwrap().entry("Drafts".into()).or_default().push(env(9, "From the phone", true));
    let drafts_mb = engine.store().mailbox_by_role(id, MailboxRole::Drafts).unwrap().unwrap().id;
    engine.handle(Command::SyncMailbox(drafts_mb));
    engine.settle().await;
    let phone = engine.store().list(&tern_store::ListScope::Mailbox(drafts_mb), false, 5).unwrap()[0].id;

    engine.handle(Command::AdoptDraft { key: "k".into(), account: id, message: phone });
    engine.handle(Command::SaveDraft { key: "k".into(), account: id, draft: draft("Edited here") });
    engine.settle().await;
    assert_eq!(server_drafts(&fake), ["Edited here"], "the phone's copy was replaced");

    // Opening another phone draft and sending it without changes removes it too.
    fake.boxes.lock().unwrap().entry("Drafts".into()).or_default().push(env(10, "Send as is", true));
    engine.handle(Command::SyncMailbox(drafts_mb));
    engine.settle().await;
    let list = engine.store().list(&tern_store::ListScope::Mailbox(drafts_mb), false, 5).unwrap();
    let as_is = list.iter().find(|m| m.envelope.subject == "Send as is").unwrap().id;
    engine.handle(Command::AdoptDraft { key: "k2".into(), account: id, message: as_is });
    engine.handle(Command::Send { account: id, draft: draft("Send as is"), responding_to: None, draft_key: Some("k2".into()), hold: None });
    engine.settle().await;
    assert_eq!(server_drafts(&fake), ["Edited here"]);
}

#[tokio::test]
async fn old_copy_found_by_message_id_when_append_reports_no_id() {
    let (mut engine, fake, _rx, id) = setup().await;
    *fake.append_without_id.lock().unwrap() = true;
    let save = |subject: &str| Command::SaveDraft { key: "k".into(), account: id, draft: draft(subject) };
    engine.handle(save("v1"));
    engine.settle().await;
    engine.handle(save("v2"));
    engine.settle().await;
    assert_eq!(server_drafts(&fake), ["v2"]);
}

#[tokio::test]
async fn reopening_a_saved_draft_leaves_no_stale_entry() {
    let (mut engine, fake, _rx, id) = setup().await;
    engine.handle(Command::SaveDraft { key: "first".into(), account: id, draft: draft("Plan") });
    engine.settle().await;
    let drafts_mb = engine.store().mailbox_by_role(id, MailboxRole::Drafts).unwrap().unwrap().id;
    let msg = engine.store().list(&tern_store::ListScope::Mailbox(drafts_mb), false, 5).unwrap()[0].id;

    engine.handle(Command::AdoptDraft { key: "second".into(), account: id, message: msg });
    engine.handle(Command::SaveDraft { key: "second".into(), account: id, draft: draft("Plan, edited") });
    engine.settle().await;
    assert!(engine.store().draft("first").unwrap().is_none(), "the closed window's entry is gone");
    assert_eq!(server_drafts(&fake), ["Plan, edited"]);
}

fn held_send(id: AccountId, key: &str, responding_to: Option<(MessageId, FlagChange)>) -> Command {
    Command::Send {
        account: id,
        draft: Box::new(draft_with_bcc()),
        responding_to,
        draft_key: None,
        hold: Some(Hold { key: key.into(), delay: Duration::from_secs(10) }),
    }
}

#[tokio::test(start_paused = true)]
async fn undo_send_takes_a_held_message_back() {
    let (mut engine, fake, rx, id) = setup().await;
    engine.handle(held_send(id, "w1", None));
    engine.settle().await;
    assert!(submits(&fake).is_empty(), "held, not sent yet");
    assert_eq!(engine.store().outbox().unwrap().len(), 1, "queued so a crash doesn't lose it");
    engine.flush_outbox();
    engine.settle().await;
    assert!(submits(&fake).is_empty(), "the flush leaves held messages alone");

    engine.handle(Command::UndoSend("w1".into()));
    assert!(engine.store().outbox().unwrap().is_empty());
    run_for(&mut engine, Duration::from_secs(15)).await;
    assert!(submits(&fake).is_empty(), "never sent");
    let events: Vec<Event> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert!(events.iter().any(|e| matches!(e, Event::SendUndone(k) if k == "w1")));
    assert!(!events.iter().any(|e| matches!(e, Event::SendReleased(_) | Event::Sent)));
}

#[tokio::test(start_paused = true)]
async fn held_message_is_sent_when_the_hold_ends() {
    let (mut engine, fake, rx, id) = setup().await;
    let orig = engine.store().list(&ListScope::Mailbox(inbox(&engine, id)), false, 50).unwrap()[0].id;
    engine.handle(held_send(id, "w2", Some((orig, FlagChange::Answered))));
    run_for(&mut engine, Duration::from_secs(5)).await;
    assert!(submits(&fake).is_empty());
    run_for(&mut engine, Duration::from_secs(10)).await;
    assert_eq!(submits(&fake).len(), 1);
    assert!(engine.store().outbox().unwrap().is_empty());
    assert!(engine.store().message(orig).unwrap().envelope.flags.answered, "reply flag survives the hold");
    let events: Vec<Event> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert!(events.iter().any(|e| matches!(e, Event::SendReleased(k) if k == "w2")));

    // Too late to undo: nothing happens.
    engine.handle(Command::UndoSend("w2".into()));
    assert!(!std::iter::from_fn(|| rx.try_recv().ok()).any(|e| matches!(e, Event::SendUndone(_))));
}

#[tokio::test(start_paused = true)]
async fn message_held_when_tern_quit_is_sent_on_next_start() {
    let (mut engine, fake, _rx, id) = setup().await;
    engine.handle(held_send(id, "w3", None));
    engine.settle().await;
    assert!(submits(&fake).is_empty());
    // The next start: the outbox row is still there, the in-memory hold isn't.
    engine.held.clear();
    engine.flush_outbox();
    engine.settle().await;
    assert_eq!(submits(&fake).len(), 1);
}
