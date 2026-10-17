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
use tern_core::backend::{MailBackend, MailboxDelta, PushEvent};
use tern_core::model::*;
use tern_core::{Error, Result};
use tern_imap::ImapBackend;
use tern_jmap::JmapBackend;
use tern_store::Store;
use tokio::sync::mpsc;
use tracing::{info, warn};

pub use config::Paths;

/// How many messages per mailbox to download on first sync.
pub const INITIAL_SYNC_LIMIT: u32 = 1000;
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

#[derive(Debug)]
pub enum Command {
    /// "Get Mail" (⇧⌘N).
    SyncAll,
    SyncMailbox(MailboxId),
    /// Download a body if it isn't cached; emits `BodyReady`.
    FetchBody(MessageId),
    SetFlags(Vec<MessageId>, FlagChange),
    Move(Vec<MessageId>, MoveTarget),
    Send {
        account: AccountId,
        draft: Box<Draft>,
        /// Mark this message \Answered / $Forwarded once sent.
        responding_to: Option<(MessageId, FlagChange)>,
    },
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
    Error { account: Option<AccountId>, message: String },
}

enum Internal {
    Mailboxes(AccountId, Vec<RemoteMailbox>),
    Delta(AccountId, MailboxId, MailboxDelta),
    Body(MessageId, MessageBody),
    Push(AccountId, PushEvent),
    Sent { outbox_id: i64, account: AccountId, responding_to: Option<(MessageId, FlagChange)> },
    /// A queued send failed; it stays in the outbox for the next flush.
    SendFailed { outbox_id: i64, account: AccountId, error: Error },
    Failed { account: Option<AccountId>, mailbox: Option<MailboxId>, error: Error },
    /// A (re)built backend for an account, ready to start syncing. `reconnect`
    /// is set for rebuilds of an existing account (dropped if it was removed meanwhile).
    AccountReady { cfg: AccountConfig, backend: Arc<dyn MailBackend>, reconnect: bool },
    /// Building the backend failed (keyring, OAuth refresh); try again later.
    ReconnectFailed(AccountConfig, Error),
    /// A task finished with nothing to report.
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
    auth_retry: HashMap<AccountId, AuthRetry>,
    connect: Connect,
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
            auth_retry: Default::default(),
            connect: Arc::new(|cfg| Box::pin(async move { backend_for(&cfg).await })),
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

    /// Force a full re-read of one mailbox (drops the cursor).
    fn store_resync(&mut self, mailbox: MailboxId) {
        let _ = self.store.apply_delta(mailbox, &MailboxDelta { full_resync: false, ..Default::default() });
        self.sync_mailbox(mailbox);
    }

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::SyncAll => self.sync_all(),
            Command::SyncMailbox(m) => self.sync_mailbox(m),
            Command::FetchBody(id) => self.fetch_body(id),
            Command::SetFlags(ids, change) => self.set_flags(ids, change),
            Command::Move(ids, target) => self.move_messages(ids, target),
            Command::Send { account, draft, responding_to } => self.send(account, *draft, responding_to),
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
        for ((account, mb_remote, mailbox), items) in groups {
            let Some(acct) = self.accounts.get(&account) else { continue };
            let backend = acct.backend.clone();
            self.spawn_task(account, async move {
                // Different messages may end with different flag sets; send each distinct set once.
                let mut by_flags: Vec<(Flags, Vec<String>)> = Vec::new();
                for (rid, f) in items {
                    match by_flags.iter_mut().find(|(g, _)| *g == f) {
                        Some((_, v)) => v.push(rid),
                        None => by_flags.push((f, vec![rid])),
                    }
                }
                for (f, rids) in by_flags {
                    if let Err(error) = backend.set_flags(&mb_remote, &rids, &f).await {
                        return Internal::Failed { account: Some(account), mailbox: Some(mailbox), error };
                    }
                }
                Internal::Nothing
            });
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
            let Some(acct) = self.accounts.get(&account) else { continue };
            let backend = acct.backend.clone();
            let dest_remote = dest.remote.remote_id.clone();
            let rids: Vec<String> = items.iter().map(|(_, r)| r.clone()).collect();
            local_delete.extend(items.iter().map(|(id, _)| *id));
            self.spawn_task(account, async move {
                let r = if permanent {
                    let deleted = Flags { deleted: true, seen: true, ..Default::default() };
                    match backend.set_flags(&from_remote, &rids, &deleted).await {
                        Ok(()) => backend.expunge(&from_remote).await,
                        Err(e) => Err(e),
                    }
                } else {
                    backend.move_messages(&from_remote, &rids, &dest_remote).await.map(|_| ())
                };
                match r {
                    // Pull the destination so the moved mail shows up there.
                    Ok(()) if !permanent => Internal::Push(account, PushEvent::MailboxChanged { remote_id: dest_remote }),
                    Ok(()) => Internal::Nothing,
                    // The local copy is already gone; resync the source to restore it.
                    Err(error) => Internal::Failed { account: Some(account), mailbox: Some(from_id), error },
                }
            });
        }
        if let Err(e) = self.store.delete_local(&local_delete) {
            self.fail(None, e);
        }
        self.emit(Event::MessagesChanged(None));
        self.emit(Event::MailboxesChanged);
    }

    fn send(&mut self, account: AccountId, mut draft: Draft, responding_to: Option<(MessageId, FlagChange)>) {
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
        self.dispatch_outgoing(outbox_id, account, out, responding_to);
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

    /// Retry anything left in the outbox (previous run crashed or was offline).
    fn flush_outbox(&mut self) {
        let Ok(items) = self.store.outbox() else { return };
        for item in items {
            if self.sending.contains(&item.id) {
                continue;
            }
            let (id, account) = (item.id, item.account);
            let Some(out) = outgoing_from_item(item) else { continue };
            self.dispatch_outgoing(id, account, out, None);
        }
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
                if let Err(e) = self.store.outbox_failed(outbox_id, &error.to_string()) {
                    self.fail(Some(account), e);
                }
                self.internal(Internal::Failed { account: Some(account), mailbox: None, error });
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
