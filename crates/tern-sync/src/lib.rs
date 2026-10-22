//! The sync engine.
//!
//! Runs on its own thread with a Tokio runtime. The UI sends [`Command`]s and
//! receives [`Event`]s over `async-channel` (which works from both the GLib main
//! loop and Tokio). The engine is the only writer to the store; the UI reads
//! through its own connection and refreshes when told something changed.
//!
//! Network calls run in spawned tasks; their results come back to the engine
//! loop as [`Internal`] messages so all store writes stay on one task.

pub mod config;
pub mod demo;

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tern_core::account::{AccountConfig, Provider, Transport};
use tern_core::backend::{MailBackend, MailboxDelta, OlderBatch, OlderFrom, PushEvent};
use tern_core::model::*;
use tern_core::{Error, Result};
use tern_imap::ImapBackend;
use tern_jmap::JmapBackend;
use serde::{Deserialize, Serialize};
use tern_store::{ServerCopy, Store};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

pub use config::Paths;

/// How many messages per mailbox to download on first sync.
pub const INITIAL_SYNC_LIMIT: u32 = 1000;
/// How many older messages to download per mailbox when scrolling back.
pub const OLDER_BATCH: u32 = 250;
/// Background resync of every mailbox (push only watches the inbox).
pub const PERIODIC_SYNC: Duration = Duration::from_secs(5 * 60);
/// IMAP IDLE must be re-issued before 29 minutes (RFC 2177).
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(25 * 60);
/// Backoff between reconnects after repeated sign-in failures.
pub const AUTH_RETRY_MIN: Duration = Duration::from_secs(30);
pub const AUTH_RETRY_MAX: Duration = Duration::from_secs(30 * 60);

/// Wait before reconnect attempt `n` (1-based) in a run of sign-in failures.
/// The first is immediate (usually an expired token that a refresh fixes); after
/// that it doubles from `AUTH_RETRY_MIN` up to `AUTH_RETRY_MAX`, so a wrong
/// password doesn't hammer the server or flood the UI with errors.
pub fn auth_retry_delay(n: u32) -> Duration {
    match n {
        0 | 1 => Duration::ZERO,
        n => AUTH_RETRY_MIN.saturating_mul(1 << (n - 2).min(16)).min(AUTH_RETRY_MAX),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveTarget {
    Trash,
    Archive,
    Junk,
    Inbox,
    Mailbox(MailboxId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagChange {
    Seen(bool),
    Flag(Option<FlagColor>),
    Answered,
    Forwarded,
}

/// A queued message action, stored as JSON in `pending_ops` until the server
/// accepts it. Keyed by `(mailbox, remote id)` because message rows are deleted
/// and re-inserted by syncs; flags are the full resulting set, so replay is idempotent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Op {
    SetFlags { mailbox: MailboxId, mailbox_remote: String, items: Vec<(String, Flags)> },
    Move { from: MailboxId, from_remote: String, to: MailboxId, to_remote: String, remote_ids: Vec<String> },
    /// Permanent delete (deleting in Trash): \Deleted + EXPUNGE.
    Destroy { mailbox: MailboxId, mailbox_remote: String, remote_ids: Vec<String> },
}

impl Op {
    /// The mailbox the action is taken in (resynced if the server rejects it).
    pub fn mailbox(&self) -> MailboxId {
        match self {
            Op::SetFlags { mailbox, .. } | Op::Destroy { mailbox, .. } => *mailbox,
            Op::Move { from, .. } => *from,
        }
    }

    async fn run(&self, backend: &dyn MailBackend) -> Result<()> {
        match self {
            Op::SetFlags { mailbox_remote, items, .. } => {
                // Different messages may end with different flag sets; send each distinct set once.
                let mut by_flags: Vec<(&Flags, Vec<String>)> = Vec::new();
                for (rid, f) in items {
                    match by_flags.iter_mut().find(|(g, _)| *g == f) {
                        Some((_, v)) => v.push(rid.clone()),
                        None => by_flags.push((f, vec![rid.clone()])),
                    }
                }
                for (f, rids) in by_flags {
                    backend.set_flags(mailbox_remote, &rids, f).await?;
                }
                Ok(())
            }
            Op::Move { from_remote, to_remote, remote_ids, .. } => {
                backend.move_messages(from_remote, remote_ids, to_remote).await.map(|_| ())
            }
            Op::Destroy { mailbox_remote, remote_ids, .. } => {
                let deleted = Flags { deleted: true, seen: true, ..Default::default() };
                backend.set_flags(mailbox_remote, remote_ids, &deleted).await?;
                backend.expunge(mailbox_remote).await
            }
        }
    }
}

/// Errors that mean "couldn't reach the server" rather than "the server said no":
/// the op stays queued and is retried on the next successful contact.
fn keeps_op(e: &Error) -> bool {
    matches!(e, Error::Network(_) | Error::Auth(_) | Error::Storage(_))
}

/// How long to hold a message before sending it, and the key to take it back with.
#[derive(Debug, Clone)]
pub struct Hold {
    pub key: String,
    pub delay: Duration,
}

/// A message waiting out its hold: queued in the outbox, not yet dispatched.
struct Held {
    outbox_id: i64,
    account: AccountId,
    out: tern_smtp::Outgoing,
    responding_to: Option<(MessageId, FlagChange)>,
}

#[derive(Debug)]
pub enum Command {
    /// "Get Mail" (⇧⌘N).
    SyncAll,
    SyncMailbox(MailboxId),
    /// Download the next batch of older mail for these mailboxes (scrolled to the end).
    LoadOlder(Vec<MailboxId>),
    /// Download a body if it isn't cached; emits `BodyReady`.
    FetchBody(MessageId),
    SetFlags(Vec<MessageId>, FlagChange),
    Move(Vec<MessageId>, MoveTarget),
    Send {
        account: AccountId,
        draft: Box<Draft>,
        /// Mark this message \Answered / $Forwarded once sent.
        responding_to: Option<(MessageId, FlagChange)>,
        /// The draft this message was saved as, removed once the message is queued.
        draft_key: Option<String>,
        /// Wait before sending so the user can take it back (`UndoSend`).
        hold: Option<Hold>,
    },
    /// Take back a message still being held (see `Send.hold`); emits `SendUndone`.
    UndoSend(String),
    /// Save a new version of the message being written under `key` (kept locally, then
    /// uploaded to the account's Drafts mailbox, replacing the previous copy).
    SaveDraft { key: String, account: AccountId, draft: Box<Draft> },
    /// A message in a Drafts mailbox was opened for editing under `key`: it becomes that
    /// draft's server copy, so saving replaces it and sending or discarding removes it.
    AdoptDraft { key: String, account: AccountId, message: MessageId },
    /// Throw a draft away, locally and on the server.
    DiscardDraft(String),
    /// Put a refused message (see `Event::SendRejected`) back in line and send it now.
    RetrySend(i64),
    /// Forget a queued or refused message without sending it.
    DiscardSend(i64),
    /// Persist a new account and start syncing it. Secrets must already be in the keyring.
    AddAccount(Box<AccountConfig>),
    RemoveAccount(AccountId),
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum Event {
    AccountsChanged,
    MailboxesChanged,
    /// Message rows changed (in this mailbox, or anywhere when `None`).
    MessagesChanged(Option<MailboxId>),
    BodyReady(MessageId),
    SyncStarted(AccountId),
    SyncFinished(AccountId),
    /// For desktop notifications and the unread badge.
    NewMail { account: AccountId, from: String, subject: String, id: MessageId },
    Sent,
    /// The largest message (encoded bytes) this account's server accepts; `None` if unknown
    /// or unlimited. Sent whenever the account connects.
    SendLimit { account: AccountId, bytes: Option<u64> },
    /// A server refused a message for good. It's kept (see `Store::outbox_rejected`) until
    /// the user edits, retries or deletes it.
    SendRejected { outbox_id: i64, account: AccountId, subject: String, reason: String },
    /// Rows were added to or removed from the refused list.
    OutboxChanged,
    /// The held message with this key was taken back and won't be sent.
    SendUndone(String),
    /// The held message with this key is on its way; it can no longer be taken back.
    SendReleased(String),
    Error { account: Option<AccountId>, message: String },
}

enum Internal {
    Mailboxes(AccountId, Vec<RemoteMailbox>),
    Delta(AccountId, MailboxId, MailboxDelta),
    Older { account: AccountId, mailbox: MailboxId, result: Result<OlderBatch> },
    Body(MessageId, MessageBody),
    Push(AccountId, PushEvent),
    Sent { outbox_id: i64, account: AccountId, responding_to: Option<(MessageId, FlagChange)> },
    /// A queued message action finished (or failed) on the server.
    OpDone { account: AccountId, id: i64, op: Op, result: Result<()> },
    /// A queued send failed; it stays in the outbox for the next flush (or, if the
    /// server refused it, is marked rejected).
    SendFailed { outbox_id: i64, account: AccountId, error: Error },
    /// Result of asking the server for its message size limit.
    SendLimit(AccountId, Option<u64>),
    /// A held message's wait is over.
    HoldOver(String),
    /// A draft version was appended to a Drafts mailbox (or failed to be).
    DraftUploaded { key: String, account: AccountId, version: i64, copy: ServerCopy, result: Result<Option<String>> },
    Failed { account: Option<AccountId>, mailbox: Option<MailboxId>, error: Error },
    /// A (re)built backend for an account, ready to start syncing. `reconnect`
    /// is set for rebuilds of an existing account (dropped if it was removed meanwhile).
    AccountReady { cfg: AccountConfig, backend: Arc<dyn MailBackend>, reconnect: bool },
    /// Building the backend failed (keyring, OAuth refresh); try again later.
    ReconnectFailed(AccountConfig, Error),
    /// A task finished with nothing to report. (Unused right now; kept as the
    /// documented return for `spawn_task` futures, see CLAUDE.md.)
    #[allow(dead_code)]
    Nothing,
    Done(AccountId),
}

struct Account {
    cfg: AccountConfig,
    backend: Arc<dyn MailBackend>,
}

type BackendFuture = Pin<Box<dyn Future<Output = Result<Arc<dyn MailBackend>>> + Send>>;
/// Builds a backend for an account; `backend_for` normally, a fake in tests.
type Connect = Arc<dyn Fn(AccountConfig) -> BackendFuture + Send + Sync>;

/// Sign-in failure streak for one account.
#[derive(Debug, Default)]
struct AuthRetry {
    failures: u32,
    /// A reconnect is scheduled or running; further failures don't add another.
    pending: bool,
}

/// Handle the UI keeps.
#[derive(Clone)]
pub struct EngineHandle {
    pub commands: async_channel::Sender<Command>,
    pub events: async_channel::Receiver<Event>,
    pub paths: Paths,
}

impl EngineHandle {
    pub fn send(&self, c: Command) {
        if self.commands.try_send(c).is_err() {
            warn!("engine is not running");
        }
    }
}

/// Start the engine on a background thread.
pub fn spawn(paths: Paths) -> Result<EngineHandle> {
    let (cmd_tx, cmd_rx) = async_channel::unbounded();
    let (ev_tx, ev_rx) = async_channel::unbounded();
    let p = paths.clone();
    std::thread::Builder::new()
        .name("tern-sync".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(async move {
                match Engine::open(p, ev_tx.clone()).await {
                    Ok(engine) => engine.run(cmd_rx).await,
                    Err(e) => {
                        let _ = ev_tx.send(Event::Error { account: None, message: e.to_string() }).await;
                    }
                }
            });
        })
        .map_err(|e| Error::Config(e.to_string()))?;
    Ok(EngineHandle { commands: cmd_tx, events: ev_rx, paths })
}

/// A JMAP backend for credential checks (the app doesn't depend on tern-jmap directly).
pub fn jmap_backend(session_url: String, token: String) -> Arc<dyn MailBackend> {
    Arc::new(JmapBackend::new(session_url, token))
}

pub async fn backend_for(cfg: &AccountConfig) -> Result<Arc<dyn MailBackend>> {
    let cred = tern_auth::credential_for(cfg).await?;
    Ok(match &cfg.transport {
        Transport::Imap { imap, .. } => Arc::new(ImapBackend::new(imap.clone(), cred)),
        Transport::Jmap { session_url } => match cred {
            tern_auth::Credential::Bearer { token } => Arc::new(JmapBackend::new(session_url.clone(), token)),
            _ => return Err(Error::Config("JMAP accounts need an API token".into())),
        },
    })
}

pub struct Engine {
    paths: Option<Paths>,
    store: Store,
    accounts: HashMap<AccountId, Account>,
    events: async_channel::Sender<Event>,
    tx: mpsc::UnboundedSender<Internal>,
    rx: mpsc::UnboundedReceiver<Internal>,
    /// Mailboxes with a sync task in flight, so "Get Mail" spam doesn't stack.
    syncing: std::collections::HashSet<MailboxId>,
    in_flight: HashMap<AccountId, usize>,
    /// Outbox rows with a send in flight, so a flush doesn't send them twice.
    sending: std::collections::HashSet<i64>,
    /// Messages waiting out their Undo Send hold, by hold key.
    held: HashMap<String, Held>,
    /// Each account's message size limit, when the server told us.
    send_limits: HashMap<AccountId, u64>,
    /// Drafts with an upload in flight (one at a time per draft).
    drafts_uploading: std::collections::HashSet<String>,
    auth_retry: HashMap<AccountId, AuthRetry>,
    connect: Connect,
    /// Accounts with a queued op in flight; ops run one at a time per account.
    draining: std::collections::HashSet<AccountId>,
    /// Mailboxes loading older mail (these also hold their `syncing` slot).
    loading_older: std::collections::HashSet<MailboxId>,
    /// "Load older" asked for while the mailbox was syncing; runs after it.
    older_wanted: std::collections::HashSet<MailboxId>,
    /// A sync asked for while the mailbox was loading older mail; runs after it.
    sync_wanted: std::collections::HashSet<MailboxId>,
    /// The server had nothing older (this session).
    exhausted: std::collections::HashSet<MailboxId>,
    older_batch: u32,
}

impl Engine {
    async fn open(paths: Paths, events: async_channel::Sender<Event>) -> Result<Self> {
        let store = Store::open(&paths.database)?;
        let file = paths.load_accounts()?;
        let mut engine = Self::new(store, events);
        engine.paths = Some(paths);
        for cfg in file.accounts.into_iter().filter(|a| a.enabled) {
            match backend_for(&cfg).await {
                Ok(b) => engine.add(cfg, b),
                Err(e) => engine.emit(Event::Error { account: Some(cfg.id), message: e.to_string() }),
            }
        }
        Ok(engine)
    }

    /// Engine with no accounts and no config file (tests, demo mode).
    pub fn new(store: Store, events: async_channel::Sender<Event>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            paths: None,
            store,
            accounts: HashMap::new(),
            events,
            tx,
            rx,
            syncing: Default::default(),
            in_flight: Default::default(),
            sending: Default::default(),
            held: Default::default(),
            send_limits: Default::default(),
            drafts_uploading: Default::default(),
            auth_retry: Default::default(),
            connect: Arc::new(|cfg| Box::pin(async move { backend_for(&cfg).await })),
            draining: Default::default(),
            loading_older: Default::default(),
            older_wanted: Default::default(),
            sync_wanted: Default::default(),
            exhausted: Default::default(),
            older_batch: OLDER_BATCH,
        }
    }

    pub fn add(&mut self, cfg: AccountConfig, backend: Arc<dyn MailBackend>) {
        self.accounts.insert(cfg.id, Account { cfg, backend });
    }

    fn emit(&self, e: Event) {
        let _ = self.events.try_send(e);
    }

    pub async fn run(mut self, commands: async_channel::Receiver<Command>) {
        for id in self.accounts.keys().copied().collect::<Vec<_>>() {
            self.start_account(id);
        }
        self.flush_outbox();
        let mut tick = tokio::time::interval(PERIODIC_SYNC);
        tick.tick().await; // first tick is immediate; we just synced.
        loop {
            tokio::select! {
                cmd = commands.recv() => match cmd {
                    Ok(Command::Shutdown) | Err(_) => break,
                    Ok(cmd) => self.handle(cmd),
                },
                Some(msg) = self.rx.recv() => self.internal(msg),
                _ = tick.tick() => {
                    self.sync_all();
                    self.flush_outbox();
                }
            }
        }
        info!("sync engine stopped");
    }

    /// Process queued internal results without blocking (tests drive the engine with this).
    pub async fn pump(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            self.internal(msg);
        }
    }

    /// Wait until every spawned task for every account has reported back.
    pub async fn settle(&mut self) {
        loop {
            self.pump().await;
            if self.in_flight.values().all(|&n| n == 0) {
                break;
            }
            if let Some(msg) = self.rx.recv().await {
                self.internal(msg);
            }
        }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    fn start_account(&mut self, id: AccountId) {
        self.refresh_mailboxes(id);
        self.watch_inbox(id);
        self.probe_send_limit(id);
    }

    /// Ask the account's sending server how large a message it accepts: the JMAP session for
    /// backends that submit mail themselves, otherwise the SMTP server's EHLO `SIZE`.
    fn probe_send_limit(&mut self, id: AccountId) {
        let Some(acct) = self.accounts.get(&id) else { return };
        let backend = acct.backend.clone();
        let cfg = acct.cfg.clone();
        self.spawn_task(id, async move {
            let limit = if backend.handles_submission() {
                backend.send_limit().await
            } else if let Transport::Imap { smtp, .. } = &cfg.transport {
                tern_smtp::size_limit(smtp).await
            } else {
                Ok(None)
            };
            match limit {
                Ok(bytes) => Internal::SendLimit(id, bytes),
                Err(e) => {
                    // Not worth a notice: without a limit the server's answer decides.
                    debug!(%id, "couldn't learn the send size limit: {e}");
                    Internal::SendLimit(id, None)
                }
            }
        });
    }

    fn retry_send(&mut self, id: i64) {
        if let Err(e) = self.store.outbox_retry(id) {
            return self.fail(None, e);
        }
        self.emit(Event::OutboxChanged);
        match self.store.outbox_item(id) {
            Ok(Some(item)) => {
                let account = item.account;
                if let Some(out) = outgoing_from_item(item) {
                    self.dispatch_outgoing(id, account, out, None);
                }
            }
            Ok(None) => {}
            Err(e) => self.fail(None, e),
        }
    }

    fn spawn_task(&mut self, account: AccountId, fut: impl Future<Output = Internal> + Send + 'static) {
        *self.in_flight.entry(account).or_default() += 1;
        if self.in_flight[&account] == 1 {
            self.emit(Event::SyncStarted(account));
        }
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let msg = fut.await;
            let _ = tx.send(msg);
            let _ = tx.send(Internal::Done(account));
        });
    }

    fn refresh_mailboxes(&mut self, id: AccountId) {
        let Some(acct) = self.accounts.get(&id) else { return };
        let backend = acct.backend.clone();
        self.spawn_task(id, async move {
            match backend.list_mailboxes().await {
                Ok(list) => Internal::Mailboxes(id, list),
                Err(error) => Internal::Failed { account: Some(id), mailbox: None, error },
            }
        });
    }

    /// Long-lived push watcher on the inbox. Not counted as in-flight work.
    fn watch_inbox(&self, id: AccountId) {
        let Some(acct) = self.accounts.get(&id) else { return };
        let backend = acct.backend.clone();
        let tx = self.tx.clone();
        let inbox = match self.store.mailbox_by_role(id, MailboxRole::Inbox) {
            Ok(Some(m)) => m.remote.remote_id,
            _ => "INBOX".into(),
        };
        if matches!(backend.kind(), "test") {
            return;
        }
        tokio::spawn(async move {
            let mut backoff = Duration::from_secs(5);
            loop {
                match backend.wait_for_push(&inbox, IDLE_TIMEOUT).await {
                    Ok(Some(ev)) => {
                        backoff = Duration::from_secs(5);
                        if tx.send(Internal::Push(id, ev)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => backoff = Duration::from_secs(5),
                    Err(e) => {
                        warn!(%id, "push watcher: {e}; retrying in {backoff:?}");
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(Duration::from_secs(300));
                    }
                }
                if tx.is_closed() {
                    break;
                }
            }
        });
    }

    fn sync_all(&mut self) {
        for id in self.accounts.keys().copied().collect::<Vec<_>>() {
            self.refresh_mailboxes(id);
        }
    }

    fn sync_mailbox(&mut self, mailbox: MailboxId) {
        if !self.syncing.insert(mailbox) {
            // A sync already in flight covers this request; an older-mail load doesn't.
            if self.loading_older.contains(&mailbox) {
                self.sync_wanted.insert(mailbox);
            }
            return;
        }
        let Ok(mb) = self.store.mailbox(mailbox) else {
            self.syncing.remove(&mailbox);
            return;
        };
        let Some(acct) = self.accounts.get(&mb.account) else {
            self.syncing.remove(&mailbox);
            return;
        };
        let backend = acct.backend.clone();
        let since = self.store.sync_state(mailbox).ok().flatten();
        let account = mb.account;
        self.spawn_task(account, async move {
            match backend
                .sync_mailbox(&mb.remote.remote_id, since.as_ref(), Some(INITIAL_SYNC_LIMIT))
                .await
            {
                Ok(d) => Internal::Delta(account, mailbox, d),
                Err(error) => Internal::Failed { account: Some(account), mailbox: Some(mailbox), error },
            }
        });
    }

    /// Download the next batch of older mail. Shares the mailbox's `syncing` slot
    /// with regular syncs: both write the cursor, and a regular sync finishing
    /// after this with the old window would delete the older rows via `present`.
    fn load_older(&mut self, mailbox: MailboxId) {
        if self.exhausted.contains(&mailbox) || self.loading_older.contains(&mailbox) {
            return;
        }
        if self.syncing.contains(&mailbox) {
            self.older_wanted.insert(mailbox);
            return;
        }
        let Ok(mb) = self.store.mailbox(mailbox) else { return };
        let Some(backend) = self.accounts.get(&mb.account).map(|a| a.backend.clone()) else { return };
        // Never synced: there's no window to extend yet.
        let Some(since) = self.store.sync_state(mailbox).ok().flatten() else { return };
        if !mb.remote.selectable {
            return;
        }
        let (oldest_remote_id, cached) = match self.store.oldest_cached(mailbox) {
            Ok(o) => o,
            Err(e) => return self.fail(Some(mb.account), e),
        };
        let from = OlderFrom { oldest_remote_id, cached };
        self.syncing.insert(mailbox);
        self.loading_older.insert(mailbox);
        let (account, count) = (mb.account, self.older_batch);
        self.spawn_task(account, async move {
            let result = backend.fetch_older(&mb.remote.remote_id, &since, &from, count).await;
            Internal::Older { account, mailbox, result }
        });
    }

    /// Force a full re-read of one mailbox (drops the cursor).
    fn store_resync(&mut self, mailbox: MailboxId) {
        let _ = self.store.apply_delta(mailbox, &MailboxDelta { full_resync: false, ..Default::default() });
        self.sync_mailbox(mailbox);
    }

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::SyncAll => self.sync_all(),
            Command::SyncMailbox(m) => self.sync_mailbox(m),
            Command::LoadOlder(boxes) => boxes.into_iter().for_each(|m| self.load_older(m)),
            Command::FetchBody(id) => self.fetch_body(id),
            Command::SetFlags(ids, change) => self.set_flags(ids, change),
            Command::Move(ids, target) => self.move_messages(ids, target),
            Command::Send { account, draft, responding_to, draft_key, hold } => {
                self.send(account, *draft, responding_to, hold);
                // The outbox holds the message now; the draft has done its job.
                if let Some(key) = draft_key {
                    self.discard_draft(&key);
                }
            }
            Command::SaveDraft { key, account, draft } => self.save_draft(key, account, *draft),
            Command::AdoptDraft { key, account, message } => self.adopt_draft(&key, account, message),
            Command::DiscardDraft(key) => self.discard_draft(&key),
            Command::UndoSend(key) => self.undo_send(key),
            Command::RetrySend(id) => self.retry_send(id),
            Command::DiscardSend(id) => {
                if let Err(e) = self.store.outbox_done(id) {
                    return self.fail(None, e);
                }
                self.emit(Event::OutboxChanged);
            }
            Command::AddAccount(cfg) => self.add_account(*cfg),
            Command::RemoveAccount(id) => self.remove_account(id),
            Command::Shutdown => {}
        }
    }

    fn fail(&self, account: Option<AccountId>, e: impl std::fmt::Display) {
        warn!(?account, "{e}");
        self.emit(Event::Error { account, message: e.to_string() });
    }

    fn fetch_body(&mut self, id: MessageId) {
        if matches!(self.store.body(id), Ok(Some(_))) {
            self.emit(Event::BodyReady(id));
            return;
        }
        let (account, mailbox, rid) = match self.store.remote_ref(id) {
            Ok(r) => r,
            Err(e) => return self.fail(None, e),
        };
        let Some(acct) = self.accounts.get(&account) else { return };
        let backend = acct.backend.clone();
        self.spawn_task(account, async move {
            match backend.fetch_body(&mailbox, &rid).await {
                Ok(b) => Internal::Body(id, b),
                Err(error) => Internal::Failed { account: Some(account), mailbox: None, error },
            }
        });
    }

    fn set_flags(&mut self, ids: Vec<MessageId>, change: FlagChange) {
        // Group by (account, mailbox) and apply optimistically.
        let mut groups: HashMap<(AccountId, String, MailboxId), Vec<(String, Flags)>> = HashMap::new();
        for id in ids {
            let Ok(msg) = self.store.message(id) else { continue };
            let Ok((account, mb_remote, rid)) = self.store.remote_ref(id) else { continue };
            let mut f = msg.envelope.flags.clone();
            match change {
                FlagChange::Seen(s) => f.seen = s,
                FlagChange::Flag(c) => f.flag = c,
                FlagChange::Answered => f.answered = true,
                FlagChange::Forwarded => f.forwarded = true,
            }
            if let Err(e) = self.store.set_flags(id, &f) {
                self.fail(Some(account), e);
            }
            groups.entry((account, mb_remote, msg.mailbox)).or_default().push((rid, f));
        }
        self.emit(Event::MessagesChanged(None));
        self.emit(Event::MailboxesChanged); // unread / flagged badges
        for ((account, mailbox_remote, mailbox), items) in groups {
            self.enqueue(account, Op::SetFlags { mailbox, mailbox_remote, items });
        }
    }

    fn move_messages(&mut self, ids: Vec<MessageId>, target: MoveTarget) {
        let mut groups: HashMap<(AccountId, String, MailboxId), Vec<(MessageId, String)>> = HashMap::new();
        for id in &ids {
            let Ok(msg) = self.store.message(*id) else { continue };
            let Ok((account, mb_remote, rid)) = self.store.remote_ref(*id) else { continue };
            groups.entry((account, mb_remote, msg.mailbox)).or_default().push((*id, rid));
        }
        let mut local_delete = Vec::new();
        let mut ops = Vec::new();
        for ((account, from_remote, from_id), items) in groups {
            let dest = match target {
                MoveTarget::Mailbox(m) => self.store.mailbox(m).ok(),
                MoveTarget::Trash => self.store.mailbox_by_role(account, MailboxRole::Trash).ok().flatten(),
                MoveTarget::Archive => self
                    .store
                    .mailbox_by_role(account, MailboxRole::Archive)
                    .ok()
                    .flatten()
                    .or_else(|| self.store.mailbox_by_role(account, MailboxRole::All).ok().flatten()),
                MoveTarget::Junk => self.store.mailbox_by_role(account, MailboxRole::Junk).ok().flatten(),
                MoveTarget::Inbox => self.store.mailbox_by_role(account, MailboxRole::Inbox).ok().flatten(),
            };
            let Some(dest) = dest else {
                self.fail(Some(account), format!("no {target:?} mailbox for this account"));
                continue;
            };
            // Deleting from Trash permanently removes, as in Apple Mail.
            let permanent = target == MoveTarget::Trash && dest.id == from_id;
            if dest.id == from_id && !permanent {
                continue;
            }
            let remote_ids: Vec<String> = items.iter().map(|(_, r)| r.clone()).collect();
            local_delete.extend(items.iter().map(|(id, _)| *id));
            ops.push((
                account,
                if permanent {
                    Op::Destroy { mailbox: from_id, mailbox_remote: from_remote, remote_ids }
                } else {
                    Op::Move {
                        from: from_id,
                        from_remote,
                        to: dest.id,
                        to_remote: dest.remote.remote_id.clone(),
                        remote_ids,
                    }
                },
            ));
        }
        if let Err(e) = self.store.delete_local(&local_delete) {
            self.fail(None, e);
        }
        self.emit(Event::MessagesChanged(None));
        self.emit(Event::MailboxesChanged);
        for (account, op) in ops {
            self.enqueue(account, op);
        }
    }

    /// Queue an action durably, then try to deliver the account's queue.
    fn enqueue(&mut self, account: AccountId, op: Op) {
        let json = match serde_json::to_string(&op) {
            Ok(j) => j,
            Err(e) => return self.fail(Some(account), e),
        };
        if let Err(e) = self.store.pending_push(account, &json) {
            return self.fail(Some(account), e);
        }
        self.drain(account);
    }

    /// The account's queued ops in order, skipping any that don't parse.
    fn queued_ops(&self, account: AccountId) -> Vec<(i64, Op)> {
        self.store
            .pending_ops(account)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(id, json)| serde_json::from_str(&json).ok().map(|op| (id, op)))
            .collect()
    }

    /// Re-apply queued ops for `mailbox` over fresh server data, so a sync doesn't
    /// revert changes the server hasn't received yet.
    fn overlay_pending(&mut self, account: AccountId, mailbox: MailboxId) {
        for (_, op) in self.queued_ops(account) {
            if op.mailbox() != mailbox {
                continue;
            }
            let r = match &op {
                Op::SetFlags { items, .. } => items
                    .iter()
                    .try_for_each(|(rid, f)| self.store.set_flags_remote(mailbox, rid, f)),
                Op::Move { remote_ids, .. } | Op::Destroy { remote_ids, .. } => {
                    self.store.delete_remote(mailbox, remote_ids)
                }
            };
            if let Err(e) = r {
                self.fail(Some(account), e);
            }
        }
    }

    /// The server renumbered `mailbox`: queued ops there point at the wrong
    /// messages now, so drop them rather than apply them.
    fn discard_ops_for(&mut self, account: AccountId, mailbox: MailboxId) {
        let stale: Vec<i64> = self
            .queued_ops(account)
            .into_iter()
            .filter(|(_, op)| op.mailbox() == mailbox)
            .map(|(id, _)| id)
            .collect();
        if stale.is_empty() {
            return;
        }
        for id in &stale {
            let _ = self.store.pending_done(*id);
        }
        let name = self.store.mailbox(mailbox).map(|m| m.remote.name).unwrap_or_default();
        self.fail(
            Some(account),
            format!(
                "{} queued change(s) in {name} were discarded because the server renumbered its messages",
                stale.len()
            ),
        );
    }

    /// Send the account's oldest queued op, unless one is already in flight.
    /// `OpDone` continues with the next one.
    fn drain(&mut self, account: AccountId) {
        if self.draining.contains(&account) {
            return;
        }
        let Some(backend) = self.accounts.get(&account).map(|a| a.backend.clone()) else { return };
        let next = match self.store.pending_ops(account) {
            Ok(ops) => ops.into_iter().next(),
            Err(e) => return self.fail(Some(account), e),
        };
        let Some((id, json)) = next else { return };
        let op: Op = match serde_json::from_str(&json) {
            Ok(op) => op,
            Err(e) => {
                warn!(%account, "dropping unreadable queued op {id}: {e}");
                let _ = self.store.pending_done(id);
                return self.drain(account);
            }
        };
        self.draining.insert(account);
        self.spawn_task(account, async move {
            let result = op.run(backend.as_ref()).await;
            Internal::OpDone { account, id, op, result }
        });
    }

    fn send(
        &mut self,
        account: AccountId,
        mut draft: Draft,
        responding_to: Option<(MessageId, FlagChange)>,
        hold: Option<Hold>,
    ) {
        let Some(acct) = self.accounts.get(&account) else {
            return self.fail(Some(account), "unknown account");
        };
        if draft.from.is_none() {
            draft.from = Some(acct.cfg.identity.clone());
        }
        let out = match tern_smtp::build(&draft) {
            Ok(o) => o,
            Err(e) => return self.fail(Some(account), e),
        };
        // Queue first so a crash or offline send isn't lost. The envelope is
        // stored too: it's the only place Bcc recipients exist.
        let from = out.envelope.from().map(|a| a.to_string()).unwrap_or_default();
        let rcpts: Vec<String> = out.envelope.to().iter().map(|a| a.to_string()).collect();
        let outbox_id = match self.store.outbox_push(account, &out.raw, &from, &rcpts) {
            Ok(id) => id,
            Err(e) => return self.fail(Some(account), e),
        };
        match hold {
            Some(Hold { key, delay }) => {
                self.held.insert(key.clone(), Held { outbox_id, account, out, responding_to });
                // Not a spawn_task: waiting isn't work in flight (no spinner, `settle` doesn't wait).
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(delay).await;
                    let _ = tx.send(Internal::HoldOver(key));
                });
            }
            None => self.dispatch_outgoing(outbox_id, account, out, responding_to),
        }
    }

    fn undo_send(&mut self, key: String) {
        // Already released: too late, and the UI was told so by `SendReleased`.
        let Some(h) = self.held.remove(&key) else { return };
        if let Err(e) = self.store.outbox_done(h.outbox_id) {
            return self.fail(Some(h.account), e);
        }
        self.emit(Event::SendUndone(key));
    }

    fn release(&mut self, key: String) {
        let Some(h) = self.held.remove(&key) else { return };
        self.emit(Event::SendReleased(key));
        self.dispatch_outgoing(h.outbox_id, h.account, h.out, h.responding_to);
    }

    fn dispatch_outgoing(
        &mut self,
        outbox_id: i64,
        account: AccountId,
        out: tern_smtp::Outgoing,
        responding_to: Option<(MessageId, FlagChange)>,
    ) {
        let Some(acct) = self.accounts.get(&account) else { return };
        if !self.sending.insert(outbox_id) {
            return;
        }
        // Don't upload what the server has said it won't take (e.g. queued before the
        // limit was known); refuse it the same way the server would.
        if let Some(&limit) = self.send_limits.get(&account)
            && out.raw.len() as u64 > limit
        {
            let error = Error::Rejected(too_large(out.raw.len() as u64, limit));
            return self.internal(Internal::SendFailed { outbox_id, account, error });
        }
        let backend = acct.backend.clone();
        let cfg = acct.cfg.clone();
        let sent_box = self
            .store
            .mailbox_by_role(account, MailboxRole::Sent)
            .ok()
            .flatten()
            .map(|m| m.remote.remote_id);
        self.spawn_task(account, async move {
            let result: Result<()> = async {
                if backend.handles_submission() {
                    let from = out.envelope.from().map(|a| a.to_string()).unwrap_or_default();
                    let rcpts: Vec<String> = out.envelope.to().iter().map(|a| a.to_string()).collect();
                    backend.submit(&out.raw, &from, &rcpts).await
                } else {
                    let Transport::Imap { smtp, .. } = &cfg.transport else {
                        return Err(Error::Config("no SMTP server".into()));
                    };
                    let cred = tern_auth::credential_for(&cfg).await?;
                    tern_smtp::send(smtp, &cred, &out).await?;
                    // Gmail files sent mail itself; everyone else needs the copy APPENDed.
                    if cfg.provider != Provider::Gmail
                        && let Some(sent) = sent_box {
                            let seen = Flags { seen: true, ..Default::default() };
                            if let Err(e) = backend.append(&sent, &out.raw, &seen).await {
                                warn!("sent, but couldn't save to Sent: {e}");
                            }
                        }
                    Ok(())
                }
            }
            .await;
            match result {
                Ok(()) => Internal::Sent { outbox_id, account, responding_to },
                Err(error) => Internal::SendFailed { outbox_id, account, error },
            }
        });
    }

    /// Retry anything left in the outbox (previous run crashed or was offline), and drafts
    /// that haven't reached the server yet.
    fn flush_outbox(&mut self) {
        for d in self.store.drafts_pending().unwrap_or_default() {
            self.upload_draft(&d.key);
        }
        let Ok(items) = self.store.outbox() else { return };
        for item in items {
            if self.sending.contains(&item.id) || self.held.values().any(|h| h.outbox_id == item.id) {
                continue;
            }
            let (id, account) = (item.id, item.account);
            let Some(out) = outgoing_from_item(item) else { continue };
            self.dispatch_outgoing(id, account, out, None);
        }
    }

    fn adopt_draft(&mut self, key: &str, account: AccountId, message: MessageId) {
        let (Ok(msg), Ok((_, _, rid))) = (self.store.message(message), self.store.remote_ref(message)) else { return };
        let raw = self.store.body(message).ok().flatten().map(|b| b.raw).unwrap_or_default();
        let copy = ServerCopy { mailbox: msg.mailbox, remote_id: Some(rid), message_id: msg.envelope.message_id.clone() };
        let result = self
            .store
            .draft_put(key, account, &raw, msg.envelope.message_id.as_deref())
            .and_then(|()| self.store.draft_uploaded(key, &copy, 1))
            .and_then(|()| self.store.draft_forget_others(key, &copy));
        if let Err(e) = result {
            self.fail(Some(account), e);
        }
    }

    fn save_draft(&mut self, key: String, account: AccountId, mut draft: Draft) {
        let Some(acct) = self.accounts.get(&account) else {
            return self.fail(Some(account), "unknown account");
        };
        if draft.from.is_none() {
            draft.from = Some(acct.cfg.identity.clone());
        }
        let raw = match tern_smtp::build_draft(&draft) {
            Ok(r) => r,
            Err(e) => return self.fail(Some(account), e),
        };
        let message_id = header_envelope(&raw).message_id;
        if let Err(e) = self.store.draft_put(&key, account, &raw, message_id.as_deref()) {
            return self.fail(Some(account), e);
        }
        self.upload_draft(&key);
    }

    /// Put the draft's latest version on the server, if it isn't already and no upload of
    /// it is running (the running one re-uploads when it finishes if the draft changed).
    fn upload_draft(&mut self, key: &str) {
        if self.drafts_uploading.contains(key) {
            return;
        }
        let Ok(Some(row)) = self.store.draft(key) else { return };
        if row.uploaded {
            return;
        }
        let Some(acct) = self.accounts.get(&row.account) else { return };
        let Ok(Some(drafts)) = self.store.mailbox_by_role(row.account, MailboxRole::Drafts) else {
            debug!(account = %row.account, "no Drafts mailbox; keeping the draft on this computer");
            return;
        };
        let backend = acct.backend.clone();
        self.drafts_uploading.insert(key.to_string());
        let (key, account, version) = (key.to_string(), row.account, row.version);
        let copy = ServerCopy { mailbox: drafts.id, remote_id: None, message_id: row.message_id.clone() };
        self.spawn_task(account, async move {
            let flags = Flags { seen: true, draft: true, ..Default::default() };
            let result = backend.append(&drafts.remote.remote_id, &row.raw, &flags).await;
            Internal::DraftUploaded { key, account, version, copy, result }
        });
    }

    fn draft_uploaded(&mut self, key: String, account: AccountId, version: i64, mut copy: ServerCopy, result: Result<Option<String>>) {
        self.drafts_uploading.remove(&key);
        let remote_id = match result {
            Ok(id) => id,
            // Offline: the local copy stays pending and the periodic flush tries again.
            Err(Error::Network(e)) => return debug!(%key, "draft not uploaded yet: {e}"),
            Err(e) => return self.fail(Some(account), e),
        };
        copy.remote_id = remote_id;
        let mailbox = copy.mailbox;
        match self.store.draft(&key) {
            // Discarded (or sent) while uploading: this copy isn't wanted either.
            Ok(None) => self.delete_server_copy(account, &copy),
            Ok(Some(row)) => {
                if let Err(e) = self.store.draft_uploaded(&key, &copy, version) {
                    return self.fail(Some(account), e);
                }
                if let Some(old) = row.remote.filter(|old| *old != copy) {
                    self.delete_server_copy(account, &old);
                }
                if row.version != version {
                    self.upload_draft(&key);
                }
            }
            Err(e) => self.fail(Some(account), e),
        }
        self.sync_mailbox(mailbox);
    }

    fn discard_draft(&mut self, key: &str) {
        let Ok(Some(row)) = self.store.draft(key) else { return };
        if let Err(e) = self.store.draft_delete(key) {
            return self.fail(Some(row.account), e);
        }
        if let Some(copy) = row.remote {
            self.delete_server_copy(row.account, &copy);
        }
    }

    /// Remove a draft copy: its local row now, the server message through the durable queue.
    /// Without a known remote id, look it up by Message-ID in what's cached; if it isn't
    /// cached yet, one stale copy can remain.
    fn delete_server_copy(&mut self, account: AccountId, copy: &ServerCopy) {
        let rid = copy.remote_id.clone().or_else(|| {
            let mid = copy.message_id.as_deref()?;
            self.store.find_by_message_id(copy.mailbox, mid).ok().flatten().map(|(_, rid)| rid)
        });
        let Some(rid) = rid else {
            return warn!(%account, "couldn't find an old draft copy to delete");
        };
        let Ok(mb) = self.store.mailbox(copy.mailbox) else { return };
        // The copy belongs to its mailbox's account, which differs from the draft's if the
        // From account was changed while writing.
        let account = mb.account;
        if let Err(e) = self.store.delete_remote(copy.mailbox, std::slice::from_ref(&rid)) {
            self.fail(Some(account), e);
        }
        self.emit(Event::MessagesChanged(Some(copy.mailbox)));
        self.emit(Event::MailboxesChanged);
        self.enqueue(
            account,
            Op::Destroy { mailbox: copy.mailbox, mailbox_remote: mb.remote.remote_id, remote_ids: vec![rid] },
        );
    }

    fn add_account(&mut self, cfg: AccountConfig) {
        if let Some(paths) = &self.paths {
            let mut file = paths.load_accounts().unwrap_or_default();
            file.accounts.retain(|a| a.id != cfg.id);
            file.accounts.push(cfg.clone());
            if let Err(e) = paths.save_accounts(&file) {
                return self.fail(Some(cfg.id), e);
            }
        }
        self.auth_retry.remove(&cfg.id);
        self.rebuild_backend(cfg, Duration::ZERO, false);
        self.emit(Event::AccountsChanged);
    }

    /// Build a backend off-loop (keyring access, maybe an OAuth refresh) after
    /// `delay`, and hand it back as `AccountReady`.
    fn rebuild_backend(&self, cfg: AccountConfig, delay: Duration, reconnect: bool) {
        let tx = self.tx.clone();
        let connect = self.connect.clone();
        tokio::spawn(async move {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let _ = tx.send(match connect(cfg.clone()).await {
                Ok(backend) => Internal::AccountReady { cfg, backend, reconnect },
                Err(e) if reconnect => Internal::ReconnectFailed(cfg, e),
                Err(e) => Internal::Failed { account: Some(cfg.id), mailbox: None, error: e },
            });
        });
    }

    /// Schedule a reconnect after a sign-in failure, backing off on repeats.
    /// Failures while one is already pending (other tasks failing the same way)
    /// are absorbed. Returns whether this failure started a new attempt.
    fn schedule_reconnect(&mut self, account: AccountId) -> bool {
        let Some(cfg) = self.accounts.get(&account).map(|a| a.cfg.clone()) else { return false };
        let retry = self.auth_retry.entry(account).or_default();
        if retry.pending {
            return false;
        }
        retry.pending = true;
        retry.failures += 1;
        let delay = auth_retry_delay(retry.failures);
        if !delay.is_zero() {
            info!(%account, "sign-in failed {} times; retrying in {delay:?}", retry.failures);
        }
        self.rebuild_backend(cfg, delay, true);
        true
    }

    fn remove_account(&mut self, id: AccountId) {
        self.accounts.remove(&id);
        self.auth_retry.remove(&id);
        self.send_limits.remove(&id);
        self.draining.remove(&id);
        if let Err(e) = self.store.pending_clear(id) {
            self.fail(Some(id), e);
        }
        if let Some(paths) = &self.paths {
            let mut file = paths.load_accounts().unwrap_or_default();
            file.accounts.retain(|a| a.id != id);
            if let Err(e) = paths.save_accounts(&file) {
                self.fail(Some(id), e);
            }
        }
        if let Err(e) = self.store.sync_mailboxes(id, &[]) {
            self.fail(Some(id), e);
        }
        let _ = tern_auth::delete_secret(id);
        self.emit(Event::AccountsChanged);
        self.emit(Event::MailboxesChanged);
    }

    fn internal(&mut self, msg: Internal) {
        match msg {
            Internal::Done(account) => {
                let n = self.in_flight.entry(account).or_default();
                *n = n.saturating_sub(1);
                if *n == 0 {
                    self.emit(Event::SyncFinished(account));
                }
            }
            Internal::Mailboxes(account, list) => {
                // Talking to the server again: the sign-in failure streak is over.
                self.auth_retry.remove(&account);
                if let Err(e) = self.store.sync_mailboxes(account, &list) {
                    return self.fail(Some(account), e);
                }
                self.emit(Event::MailboxesChanged);
                let boxes = self.store.mailboxes(Some(account)).unwrap_or_default();
                for mb in boxes.into_iter().filter(|m| m.remote.selectable) {
                    self.sync_mailbox(mb.id);
                }
                // The server is reachable: deliver anything queued while it wasn't.
                self.drain(account);
            }
            Internal::Delta(account, mailbox, delta) => {
                self.syncing.remove(&mailbox);
                let initial = delta.full_resync;
                let is_inbox = self
                    .store
                    .mailbox(mailbox)
                    .is_ok_and(|m| m.remote.role == MailboxRole::Inbox);
                let new_unseen: Vec<String> = delta
                    .added
                    .iter()
                    .filter(|e| !e.flags.seen)
                    .map(|e| e.remote_id.clone())
                    .collect();
                if let Err(e) = self.store.apply_delta(mailbox, &delta) {
                    return self.fail(Some(account), e);
                }
                if delta.ids_invalidated {
                    self.discard_ops_for(account, mailbox);
                }
                self.overlay_pending(account, mailbox);
                if initial {
                    // Back to the newest window: older mail can be loaded again.
                    self.exhausted.remove(&mailbox);
                }
                if self.older_wanted.remove(&mailbox) {
                    self.load_older(mailbox);
                }
                self.emit(Event::MessagesChanged(Some(mailbox)));
                self.emit(Event::MailboxesChanged);
                if is_inbox && !initial && !new_unseen.is_empty() {
                    let rows = self
                        .store
                        .list(&tern_store::ListScope::Mailbox(mailbox), false, 200)
                        .unwrap_or_default();
                    for m in rows.iter().filter(|m| new_unseen.contains(&m.envelope.remote_id)) {
                        self.emit(Event::NewMail {
                            account,
                            from: m.envelope.from.first().map(|a| a.display().to_owned()).unwrap_or_default(),
                            subject: m.envelope.subject.clone(),
                            id: m.id,
                        });
                    }
                }
            }
            Internal::Older { account, mailbox, result } => {
                self.syncing.remove(&mailbox);
                self.loading_older.remove(&mailbox);
                match result {
                    Ok(batch) => {
                        if !batch.more {
                            self.exhausted.insert(mailbox);
                        }
                        if !batch.delta.added.is_empty() {
                            if let Err(e) = self.store.apply_delta(mailbox, &batch.delta) {
                                self.fail(Some(account), e);
                            } else {
                                self.overlay_pending(account, mailbox);
                                self.emit(Event::MessagesChanged(Some(mailbox)));
                                self.emit(Event::MailboxesChanged);
                            }
                        }
                    }
                    // Not a rollback case: nothing local changed, so no resync.
                    Err(Error::Network(e)) => info!(%account, "loading older mail: {e}"),
                    Err(e @ Error::Auth(_)) => {
                        if self.schedule_reconnect(account) {
                            self.fail(Some(account), e);
                        }
                    }
                    Err(e) => self.fail(Some(account), e),
                }
                if self.sync_wanted.remove(&mailbox) {
                    self.sync_mailbox(mailbox);
                }
            }
            Internal::Body(id, body) => {
                if let Err(e) = self.store.store_body(id, &body) {
                    return self.fail(None, e);
                }
                self.emit(Event::BodyReady(id));
            }
            Internal::Push(account, PushEvent::MailboxChanged { remote_id }) => {
                let target = self
                    .store
                    .mailboxes(Some(account))
                    .unwrap_or_default()
                    .into_iter()
                    .find(|m| m.remote.remote_id == remote_id);
                match target {
                    Some(m) => self.sync_mailbox(m.id),
                    None => self.refresh_mailboxes(account),
                }
            }
            Internal::Push(account, PushEvent::Disconnected) => self.refresh_mailboxes(account),
            Internal::AccountReady { cfg, backend, reconnect } => {
                let id = cfg.id;
                let fresh = !self.accounts.contains_key(&id);
                if reconnect {
                    if fresh {
                        return; // removed while reconnecting
                    }
                    if let Some(r) = self.auth_retry.get_mut(&id) {
                        r.pending = false;
                    }
                }
                self.add(cfg, backend);
                if fresh {
                    self.start_account(id);
                } else {
                    self.refresh_mailboxes(id);
                    self.probe_send_limit(id);
                }
            }
            Internal::ReconnectFailed(cfg, error) => {
                let account = cfg.id;
                if let Some(r) = self.auth_retry.get_mut(&account) {
                    r.pending = false;
                }
                if self.schedule_reconnect(account) {
                    self.fail(Some(account), error);
                }
            }
            Internal::OpDone { account, id, op, result } => {
                self.draining.remove(&account);
                match result {
                    Ok(()) => {
                        let _ = self.store.pending_done(id);
                        // Pull the destination so the moved mail shows up there.
                        if let Op::Move { to, .. } = op {
                            self.sync_mailbox(to);
                        }
                        self.drain(account);
                    }
                    // Unreachable: keep it queued; the next successful contact retries.
                    Err(e) if keeps_op(&e) => {
                        info!(%account, "queued change kept for later: {e}");
                        match e {
                            Error::Auth(_) => {
                                if self.schedule_reconnect(account) {
                                    self.fail(Some(account), e);
                                }
                            }
                            Error::Storage(_) => self.fail(Some(account), e),
                            _ => {}
                        }
                    }
                    // Rejected: drop it, put the mailbox back as the server has it, carry on.
                    Err(e) => {
                        let _ = self.store.pending_done(id);
                        self.store_resync(op.mailbox());
                        self.fail(Some(account), e);
                        self.drain(account);
                    }
                }
            }
            Internal::Nothing => {}
            Internal::Sent { outbox_id, account, responding_to } => {
                self.sending.remove(&outbox_id);
                let _ = self.store.outbox_done(outbox_id);
                self.emit(Event::Sent);
                if let Some((id, change)) = responding_to {
                    self.set_flags(vec![id], change);
                }
                if let Ok(Some(sent)) = self.store.mailbox_by_role(account, MailboxRole::Sent) {
                    self.sync_mailbox(sent.id);
                }
            }
            Internal::SendFailed { outbox_id, account, error } => {
                self.sending.remove(&outbox_id);
                if let Error::Rejected(reason) = &error {
                    // Sending it again won't help: keep it for the user and stop retrying.
                    if let Err(e) = self.store.outbox_reject(outbox_id, reason) {
                        return self.fail(Some(account), e);
                    }
                    let subject = self
                        .store
                        .outbox_item(outbox_id)
                        .ok()
                        .flatten()
                        .map(|i| outbox_subject(&i.raw))
                        .unwrap_or_default();
                    self.emit(Event::SendRejected { outbox_id, account, subject, reason: reason.clone() });
                    self.emit(Event::OutboxChanged);
                    return;
                }
                if let Err(e) = self.store.outbox_failed(outbox_id, &error.to_string()) {
                    self.fail(Some(account), e);
                }
                self.internal(Internal::Failed { account: Some(account), mailbox: None, error });
            }
            Internal::DraftUploaded { key, account, version, copy, result } => {
                self.draft_uploaded(key, account, version, copy, result);
            }
            Internal::HoldOver(key) => self.release(key),
            Internal::SendLimit(account, bytes) => {
                match bytes {
                    Some(n) => self.send_limits.insert(account, n),
                    None => self.send_limits.remove(&account),
                };
                self.emit(Event::SendLimit { account, bytes });
            }
            Internal::Failed { account, mailbox, error } => {
                if let Some(m) = mailbox {
                    self.syncing.remove(&m);
                    // Roll back optimistic local edits by re-reading the server
                    // (skipped for auth errors; the rebuild below resyncs).
                    if !matches!(error, Error::Auth(_)) {
                        self.store_resync(m);
                    }
                }
                if let (Some(a), Error::Auth(_)) = (account, &error) {
                    // Token may have expired: rebuild the backend with fresh credentials.
                    // Only the failure that starts a reconnect is shown, so a burst of
                    // tasks failing together doesn't stack up toasts.
                    if self.accounts.contains_key(&a) && !self.schedule_reconnect(a) {
                        return;
                    }
                }
                self.fail(account, error);
            }
        }
    }
}

/// "The message is 41.2 MB; the server accepts up to 34.2 MB."
pub fn too_large(size: u64, limit: u64) -> String {
    let mb = |b: u64| b as f64 / (1024.0 * 1024.0);
    format!("The message is {:.1} MB; the server accepts up to {:.1} MB.", mb(size), mb(limit))
}

/// Subject of a queued message, for notices.
fn outbox_subject(raw: &[u8]) -> String {
    header_envelope(raw).subject
}

/// The envelope parsed from a raw message's header.
fn header_envelope(raw: &[u8]) -> Envelope {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4).unwrap_or(raw.len());
    tern_core::mime::envelope_from_parts(String::new(), &raw[..split], None, Flags::default(), 0, None)
}

/// Turn a queued row back into an `Outgoing`, using its stored SMTP envelope.
fn outgoing_from_item(item: tern_store::OutboxItem) -> Option<tern_smtp::Outgoing> {
    let raw = item.raw;
    let (from, to) = match item.envelope {
        Some((from, to)) => (from.parse().ok()?, to.iter().filter_map(|a| a.parse().ok()).collect()),
        None => {
            // Queued before the envelope was stored: rebuild it from the headers
            // (Bcc isn't in them, so those recipients are lost).
            let env = tern_core::mime::envelope_from_parts(String::new(), &raw, None, Flags::default(), 0, None);
            let from = env.from.first()?.email.parse().ok()?;
            let to: Vec<_> = env.to.iter().chain(&env.cc).filter_map(|a| a.email.parse().ok()).collect();
            (from, to)
        }
    };
    let envelope = tern_smtp::lettre_envelope(Some(from), to).ok()?;
    Some(tern_smtp::Outgoing { raw, envelope })
}

#[cfg(test)]
mod tests;
