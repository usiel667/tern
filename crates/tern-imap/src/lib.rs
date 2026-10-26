//! IMAP4rev1 backend.
//!
//! Four connections per account: one for sync and other commands, one for opening
//! messages (so a click never waits behind a sync round, which takes ~17 s on a big Gmail
//! account), one for downloading bodies ahead in the background, and one that sits in IDLE
//! on the inbox for push. Sync is UID-based and works on any
//! server; CONDSTORE/QRESYNC are an optimisation to add later (see md/PLAN.md).

mod connect;
mod state;

use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::time::{Duration, Instant};

use async_imap::extensions::idle::IdleResponse;
use async_imap::types::{Fetch, Flag, NameAttribute, UnsolicitedResponse};
use async_trait::async_trait;
use futures::TryStreamExt;
use tern_auth::Credential;
use tern_core::account::ServerConfig;
use tern_core::backend::{MailBackend, MailboxCounts, MailboxDelta, OlderBatch, OlderFrom, PushEvent, SyncState};
use tern_core::mime::{body_from_raw, envelope_from_parts};
use tern_core::model::{Envelope, Flags, MailboxRole, MessageBody, RemoteMailbox};
use tern_core::{Error, Result};
use tokio::sync::Mutex;

pub use connect::{Session, connect};
use connect::{cmd, net};
use state::ImapState;

/// Items fetched for list rows: headers for threading + a body prefix for the preview.
const ENVELOPE_ITEMS: &str =
    "(UID FLAGS RFC822.SIZE INTERNALDATE BODY.PEEK[HEADER] BODY.PEEK[TEXT]<0.2048>)";

/// Flags this client owns; anything else on the server (e.g. `$NotJunk`) is left alone.
const MANAGED_FLAGS: &[&str] = &[
    "\\Seen", "\\Answered", "\\Draft", "\\Deleted", "\\Flagged", "$Forwarded", "$Junk",
    "$MailFlagBit0", "$MailFlagBit1", "$MailFlagBit2",
];

/// A connection unused for this long gets a NOOP before the next command.
const STALE_AFTER: Duration = Duration::from_secs(60);

/// Change-tracking extensions in use on a connection (RFC 7162).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extensions {
    pub condstore: bool,
    pub qresync: bool,
}

impl Extensions {
    pub const ALL: Self = Self { condstore: true, qresync: true };
    pub const NONE: Self = Self { condstore: false, qresync: false };
}

/// The command connection and what it negotiated.
struct Conn {
    session: Session,
    ext: Extensions,
    /// The mailbox SELECTed read-write, if any.
    selected: Option<String>,
    last_used: Instant,
}

impl Conn {
    /// SELECT `mailbox` unless it already is. Commands by UID don't need a fresh SELECT,
    /// and on Gmail each one takes a second or more.
    async fn open(&mut self, mailbox: &str) -> Result<()> {
        if self.selected.as_deref() == Some(mailbox) {
            return Ok(());
        }
        // A failed SELECT leaves nothing selected.
        self.selected = None;
        self.session.select(mailbox).await.map_err(cmd)?;
        self.selected = Some(mailbox.to_string());
        Ok(())
    }

    /// SELECT `mailbox` again even if it's open, for its current UIDVALIDITY, counts and
    /// HIGHESTMODSEQ.
    async fn reselect(&mut self, mailbox: &str, condstore: bool) -> Result<async_imap::types::Mailbox> {
        self.selected = None;
        let mb = if condstore { self.session.select_condstore(mailbox).await } else { self.session.select(mailbox).await }
            .map_err(cmd)?;
        self.selected = Some(mailbox.to_string());
        Ok(mb)
    }
}

pub struct ImapBackend {
    server: ServerConfig,
    cred: Credential,
    cmd: Mutex<Option<Conn>>,
    /// Only for `fetch_body`.
    body: Mutex<Option<Conn>>,
    /// Only for `prefetch_body`.
    prefetch: Mutex<Option<Conn>>,
    idle: Mutex<Option<Session>>,
    /// Extensions we're allowed to use even if the server offers more (tests).
    allowed: std::sync::Mutex<Extensions>,
    /// Incremental syncs per mailbox this session, for the periodic full check.
    sync_count: std::sync::Mutex<HashMap<String, u32>>,
}

impl ImapBackend {
    pub fn new(server: ServerConfig, cred: Credential) -> Self {
        Self {
            server,
            cred,
            cmd: Mutex::new(None),
            body: Mutex::new(None),
            prefetch: Mutex::new(None),
            idle: Mutex::new(None),
            allowed: std::sync::Mutex::new(Extensions::ALL),
            sync_count: Default::default(),
        }
    }

    /// Test switch: behave as if the server offered at most `ext`. Drops the
    /// current connection so the next command renegotiates.
    #[doc(hidden)]
    pub async fn limit_extensions(&self, ext: Extensions) {
        *self.allowed.lock().unwrap() = ext;
        *self.cmd.lock().await = None;
        *self.body.lock().await = None;
        *self.prefetch.lock().await = None;
    }

    /// Read-only export: every message in `mailbox` as raw RFC 822 bytes, oldest UID first,
    /// fetched `batch` at a time and handed to `each(uid, raw)`. EXAMINE and BODY.PEEK leave
    /// the server untouched (nothing marked read). Returns how many messages there were.
    pub async fn export_raw(
        &self,
        mailbox: &str,
        batch: usize,
        mut each: impl FnMut(u32, &[u8]) -> Result<()>,
    ) -> Result<usize> {
        self.with_conn(async |c| {
            // EXAMINE is read-only: STOREs would fail, so it doesn't count as selected.
            c.selected = None;
            let s = &mut c.session;
            s.examine(mailbox).await.map_err(cmd)?;
            let mut uids: Vec<u32> = s.uid_search("ALL").await.map_err(cmd)?.into_iter().collect();
            uids.sort_unstable();
            for chunk in uids.chunks(batch.max(1)) {
                let set = chunk.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
                let fetches: Vec<Fetch> = s
                    .uid_fetch(&set, "(UID BODY.PEEK[])")
                    .await
                    .map_err(cmd)?
                    .try_collect()
                    .await
                    .map_err(cmd)?;
                for f in &fetches {
                    if let (Some(uid), Some(raw)) = (f.uid, f.body()) {
                        each(uid, raw)?;
                    }
                }
            }
            Ok(uids.len())
        })
        .await
    }

    /// Test hook: the change-tracking extensions the current connection uses (what the
    /// server offers, within `limit_extensions`). `NONE` before the first command.
    #[doc(hidden)]
    pub async fn extensions(&self) -> Extensions {
        self.cmd.lock().await.as_ref().map(|c| c.ext).unwrap_or(Extensions::NONE)
    }

    /// Create a mailbox (a label on Gmail). One that already exists is fine. Used by the
    /// live tests for a scratch mailbox; mailbox management will build on it.
    pub async fn create_mailbox(&self, name: &str) -> Result<()> {
        self.with_session(async |s| match s.create(name).await {
            Ok(()) => Ok(()),
            Err(e) if e.to_string().to_ascii_uppercase().contains("ALREADYEXISTS")
                || e.to_string().to_ascii_lowercase().contains("already exists") => Ok(()),
            Err(e) => Err(cmd(e)),
        })
        .await
    }

    /// Log in and see which change-tracking extensions we can use. QRESYNC must
    /// be ENABLEd per connection; it implies CONDSTORE.
    async fn open_conn(&self) -> Result<Conn> {
        let mut session = connect(&self.server, &self.cred).await?;
        let caps = session.capabilities().await.map_err(cmd)?;
        let allowed = *self.allowed.lock().unwrap();
        let qresync = allowed.qresync && caps.has_str("QRESYNC");
        let condstore = allowed.condstore && (qresync || caps.has_str("CONDSTORE"));
        if qresync {
            session.run_command_and_check_ok("ENABLE QRESYNC").await.map_err(cmd)?;
        }
        Ok(Conn { session, ext: Extensions { condstore, qresync }, selected: None, last_used: Instant::now() })
    }

    /// Connect and log in eagerly, to validate settings in the Add Account sheet.
    pub async fn verify(&self) -> Result<()> {
        let mut s = connect(&self.server, &self.cred).await?;
        let _ = s.logout().await;
        Ok(())
    }

    /// Run `f` on the command session, connecting first if needed and dropping the
    /// connection on error so the next call reconnects.
    async fn with_session<T>(
        &self,
        f: impl AsyncFnOnce(&mut Session) -> Result<T>,
    ) -> Result<T> {
        self.with_conn(async move |c| f(&mut c.session).await).await
    }

    /// `with_session`, with access to the negotiated extensions.
    async fn with_conn<T>(&self, f: impl AsyncFnOnce(&mut Conn) -> Result<T>) -> Result<T> {
        self.on(&self.cmd, f).await
    }

    /// Run `f` on the connection in `lane`. One left unused for a while is checked with a
    /// NOOP first: Gmail drops idle connections without a word, and the command after that
    /// would fail with "connection reset" (not retried: an APPEND might have gone through).
    async fn on<T>(&self, lane: &Mutex<Option<Conn>>, f: impl AsyncFnOnce(&mut Conn) -> Result<T>) -> Result<T> {
        let mut guard = lane.lock().await;
        if let Some(c) = guard.as_mut()
            && c.last_used.elapsed() > STALE_AFTER
            && c.session.noop().await.is_err()
        {
            *guard = None;
        }
        if guard.is_none() {
            *guard = Some(self.open_conn().await?);
        }
        let conn = guard.as_mut().expect("just connected");
        let result = f(conn).await;
        conn.last_used = Instant::now();
        if matches!(result, Err(Error::Network(_) | Error::Protocol(_))) {
            *guard = None;
        }
        result
    }

    /// Download a whole message on the connection in `lane`.
    async fn body_on(&self, lane: &Mutex<Option<Conn>>, mailbox: &str, remote_id: &str) -> Result<MessageBody> {
        self.on(lane, async |c| {
            c.open(mailbox).await?;
            let s = &mut c.session;
            let fetches: Vec<Fetch> = s
                .uid_fetch(remote_id, "(UID BODY.PEEK[])")
                .await
                .map_err(cmd)?
                .try_collect()
                .await
                .map_err(cmd)?;
            let raw = fetches
                .iter()
                .find_map(|f| f.body())
                .ok_or_else(|| Error::NotFound(format!("uid {remote_id} in {mailbox}")))?;
            Ok(body_from_raw(raw.to_vec()))
        })
        .await
    }
}

/// A mailbox's role, and whether the server said so (SPECIAL-USE) rather than us guessing
/// from its name.
fn role_for(name: &str, attrs: &[NameAttribute<'_>]) -> (MailboxRole, bool) {
    if name.eq_ignore_ascii_case("INBOX") {
        return (MailboxRole::Inbox, true);
    }
    for a in attrs {
        let role = match a {
            NameAttribute::Drafts => MailboxRole::Drafts,
            NameAttribute::Sent => MailboxRole::Sent,
            NameAttribute::Junk => MailboxRole::Junk,
            NameAttribute::Trash => MailboxRole::Trash,
            NameAttribute::Archive => MailboxRole::Archive,
            NameAttribute::All => MailboxRole::All,
            NameAttribute::Flagged => MailboxRole::Flagged,
            NameAttribute::Extension(e) if e.eq_ignore_ascii_case("\\Important") => {
                MailboxRole::Important
            }
            _ => continue,
        };
        return (role, true);
    }
    // Servers without SPECIAL-USE: guess from common names, as Apple Mail does.
    let leaf = name.rsplit(['/', '.']).next().unwrap_or(name).to_ascii_lowercase();
    let role = match leaf.as_str() {
        "drafts" => MailboxRole::Drafts,
        "sent" | "sent items" | "sent messages" | "sent mail" => MailboxRole::Sent,
        "junk" | "spam" | "junk e-mail" => MailboxRole::Junk,
        "trash" | "deleted items" | "deleted messages" | "bin" => MailboxRole::Trash,
        "archive" | "archives" => MailboxRole::Archive,
        _ => MailboxRole::Custom,
    };
    (role, false)
}

/// Keep at most one mailbox per role: the one the server marked, else the first name match.
/// The rest become ordinary folders. (Gmail marks `[Gmail]/Sent Mail` and may also have old
/// labels called "Sent Messages" or "Trash" from a phone set up years ago; sending and
/// deleting must go to the marked one.)
fn one_per_role(boxes: &mut [RemoteMailbox], marked: &[bool]) {
    let mut seen = std::collections::HashSet::new();
    let order = (0..boxes.len()).filter(|&i| marked[i]).chain((0..boxes.len()).filter(|&i| !marked[i]));
    for i in order.collect::<Vec<_>>() {
        let role = boxes[i].role;
        if role != MailboxRole::Custom && !seen.insert(role) {
            boxes[i].role = MailboxRole::Custom;
        }
    }
}

fn flags_of(f: &Fetch) -> Flags {
    let strs: Vec<String> = f
        .flags()
        .map(|fl| match fl {
            Flag::Seen => "\\Seen".to_string(),
            Flag::Answered => "\\Answered".into(),
            Flag::Flagged => "\\Flagged".into(),
            Flag::Deleted => "\\Deleted".into(),
            Flag::Draft => "\\Draft".into(),
            Flag::Recent => "\\Recent".into(),
            Flag::MayCreate => "\\*".into(),
            Flag::Custom(c) => c.to_string(),
        })
        .collect();
    Flags::from_imap(strs.iter().map(String::as_str))
}

fn envelope_of(f: &Fetch) -> Option<Envelope> {
    let uid = f.uid?;
    Some(envelope_from_parts(
        uid.to_string(),
        f.header().unwrap_or_default(),
        f.text(),
        flags_of(f),
        f.size.unwrap_or(0),
        f.internal_date().map(|d| d.timestamp()),
    ))
}

fn uid_set(ids: &[String]) -> String {
    ids.join(",")
}

/// Every this-many incremental syncs of a mailbox, check every cached message
/// anyway, in case a server's change tracking misses something.
const FULL_CHECK_EVERY: u32 = 50;

fn full_check_due(nth_sync: u32) -> bool {
    nth_sync.is_multiple_of(FULL_CHECK_EVERY)
}

/// How an incremental sync checks the cached window for flag changes and deletions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncMode {
    /// FLAGS for every cached UID; deletions from what's missing (`present`).
    Full,
    /// CHANGEDSINCE for flags; UID SEARCH for `present`.
    Condstore,
    /// CHANGEDSINCE + VANISHED: only changed flags and the deleted UIDs.
    Qresync,
}

fn sync_mode(ext: Extensions, prev_modseq: u64, now_modseq: u64, full_check: bool) -> SyncMode {
    if !ext.condstore || prev_modseq == 0 || now_modseq < prev_modseq || full_check {
        SyncMode::Full
    } else if ext.qresync {
        SyncMode::Qresync
    } else {
        SyncMode::Condstore
    }
}

/// UIDs from VANISHED ranges that fall inside the cached window.
fn vanished_in_window(ranges: &[RangeInclusive<u32>], low: u32, high: u32) -> Vec<String> {
    let mut out: Vec<u32> = ranges
        .iter()
        .flat_map(|r| (*r.start()).max(low)..=(*r.end()).min(high))
        .collect();
    out.sort_unstable();
    out.dedup();
    out.into_iter().map(|u| u.to_string()).collect()
}

/// Empty the connection's unsolicited-response channel, returning any VANISHED
/// ranges in it. async-imap buffers these in a 100-slot channel and drops new
/// ones when it's full, so it must be emptied before a command whose VANISHED
/// reply we need.
fn drain_vanished(s: &mut Session) -> Vec<RangeInclusive<u32>> {
    let mut out = Vec::new();
    while let Ok(r) = s.unsolicited_responses.try_recv() {
        if let UnsolicitedResponse::Other(data) = r
            && let async_imap::imap_proto::Response::Vanished { uids, .. } = data.parsed()
        {
            out.extend(uids.iter().cloned());
        }
    }
    out
}

/// From UID SEARCH results, the newest `count` UIDs below `low_uid` (ascending),
/// and whether older ones remain after them.
fn next_older(found: impl IntoIterator<Item = u32>, low_uid: u32, count: usize) -> (Vec<u32>, bool) {
    let mut older: Vec<u32> = found.into_iter().filter(|&u| u < low_uid).collect();
    older.sort_unstable();
    let more = older.len() > count;
    let pick = older.split_off(older.len().saturating_sub(count));
    (pick, more)
}

#[async_trait]
impl MailBackend for ImapBackend {
    fn kind(&self) -> &'static str {
        "imap"
    }

    async fn list_mailboxes(&self) -> Result<Vec<RemoteMailbox>> {
        self.with_session(async |s| {
            let names: Vec<_> = s
                .list(Some(""), Some("*"))
                .await
                .map_err(cmd)?
                .try_collect()
                .await
                .map_err(cmd)?;
            let mut marked = Vec::with_capacity(names.len());
            let mut boxes: Vec<RemoteMailbox> = names
                .iter()
                .map(|n| {
                    let full = n.name().to_string();
                    let (parent, leaf) = match n.delimiter() {
                        Some(d) if full.contains(d) => {
                            let (p, l) = full.rsplit_once(d).unwrap();
                            (Some(p.to_string()), l.to_string())
                        }
                        _ => (None, full.clone()),
                    };
                    let selectable = !n.attributes().iter().any(|a| matches!(a, NameAttribute::NoSelect));
                    let (role, by_server) = role_for(&full, n.attributes());
                    marked.push(by_server);
                    RemoteMailbox {
                        role,
                        name: if full.eq_ignore_ascii_case("INBOX") { "Inbox".into() } else { leaf },
                        remote_id: full,
                        parent_remote_id: parent,
                        selectable,
                    }
                })
                .collect();
            one_per_role(&mut boxes, &marked);
            Ok(boxes)
        })
        .await
    }

    async fn sync_mailbox(
        &self,
        mailbox: &str,
        since: Option<&SyncState>,
        limit: Option<u32>,
    ) -> Result<MailboxDelta> {
        self.with_conn(async |c| {
            let ext = c.ext;
            // Counts for the whole mailbox (only part of it is cached). Asked before SELECT,
            // since STATUS on the selected mailbox is discouraged; a failure just means none.
            let counts = c
                .session
                .status(mailbox, "(MESSAGES UNSEEN)")
                .await
                .ok()
                .and_then(|st| Some(MailboxCounts { total: st.exists, unread: st.unseen? }));
            let mb = c.reselect(mailbox, ext.condstore).await?;
            let s = &mut c.session;
            let validity = mb.uid_validity.unwrap_or(0);
            // The counter as of this SELECT is what we record: anything that changes
            // while we're fetching has a higher modseq and is seen again next time.
            // 0 = unknown (no CONDSTORE, or a NOMODSEQ mailbox).
            let modseq_now = if ext.condstore { mb.highest_modseq.unwrap_or(0) } else { 0 };
            let prev = since.and_then(|st| ImapState::decode(&st.0));
            let mut delta = MailboxDelta {
                ids_invalidated: prev.as_ref().is_some_and(|p| p.uid_validity != validity),
                counts,
                ..Default::default()
            };

            match prev.filter(|p| p.uid_validity == validity) {
                // Initial sync, or the server renumbered the mailbox.
                None => {
                    delta.full_resync = true;
                    let mut added = Vec::new();
                    if mb.exists > 0 {
                        let start = limit
                            .map(|l| mb.exists.saturating_sub(l) + 1)
                            .unwrap_or(1)
                            .max(1);
                        let fetches: Vec<Fetch> = s
                            .fetch(format!("{start}:*"), ENVELOPE_ITEMS)
                            .await
                            .map_err(cmd)?
                            .try_collect()
                            .await
                            .map_err(cmd)?;
                        added = fetches.iter().filter_map(envelope_of).collect();
                    }
                    let uids = added.iter().filter_map(|e| e.remote_id.parse::<u32>().ok());
                    let st = ImapState {
                        uid_validity: validity,
                        low_uid: uids.clone().min().unwrap_or(1),
                        high_uid: uids.max().unwrap_or(0),
                        modseq: modseq_now,
                    };
                    delta.added = added;
                    delta.new_state = SyncState(st.encode());
                }
                Some(p) => {
                    let mut st = ImapState { modseq: modseq_now, ..p.clone() };
                    let nth = {
                        let mut counts = self.sync_count.lock().unwrap();
                        let n = counts.entry(mailbox.to_string()).or_default();
                        *n += 1;
                        *n
                    };
                    let mode = sync_mode(ext, p.modseq, modseq_now, full_check_due(nth));
                    // Nothing changed and no new mail: no per-message traffic at all.
                    if mode == SyncMode::Qresync
                        && modseq_now == p.modseq
                        && mb.uid_next.is_some_and(|next| next <= p.high_uid + 1)
                    {
                        delta.new_state = SyncState(st.encode());
                        return Ok(delta);
                    }
                    // New mail. `N:*` always returns at least the last message, so filter.
                    let fetches: Vec<Fetch> = s
                        .uid_fetch(format!("{}:*", p.high_uid + 1), ENVELOPE_ITEMS)
                        .await
                        .map_err(cmd)?
                        .try_collect()
                        .await
                        .map_err(cmd)?;
                    for f in &fetches {
                        if f.uid.is_some_and(|u| u > p.high_uid)
                            && let Some(e) = envelope_of(f) {
                                st.high_uid = st.high_uid.max(f.uid.unwrap());
                                delta.added.push(e);
                            }
                    }
                    // Flag changes + expunges within the window we cache.
                    if p.high_uid >= p.low_uid && p.high_uid > 0 {
                        let window = format!("{}:{}", p.low_uid, p.high_uid);
                        let query = match mode {
                            SyncMode::Full => "(UID FLAGS)".to_string(),
                            SyncMode::Condstore => format!("(UID FLAGS) (CHANGEDSINCE {})", p.modseq),
                            SyncMode::Qresync => format!("(UID FLAGS) (CHANGEDSINCE {} VANISHED)", p.modseq),
                        };
                        if mode == SyncMode::Qresync {
                            drain_vanished(s);
                        }
                        let fetches: Vec<Fetch> = s
                            .uid_fetch(&window, query)
                            .await
                            .map_err(cmd)?
                            .try_collect()
                            .await
                            .map_err(cmd)?;
                        delta.flags_changed = fetches
                            .iter()
                            .filter_map(|f| Some((f.uid?.to_string(), flags_of(f))))
                            .collect();
                        match mode {
                            SyncMode::Full => {
                                delta.present = Some(delta.flags_changed.iter().map(|(u, _)| u.clone()).collect());
                            }
                            SyncMode::Condstore => {
                                let found = s.uid_search(format!("UID {window}")).await.map_err(cmd)?;
                                delta.present = Some(found.into_iter().map(|u| u.to_string()).collect());
                            }
                            SyncMode::Qresync => {
                                delta.removed = vanished_in_window(&drain_vanished(s), p.low_uid, p.high_uid);
                            }
                        }
                    } else {
                        delta.present = Some(vec![]);
                    }
                    delta.new_state = SyncState(st.encode());
                }
            }
            Ok(delta)
        })
        .await
    }

    async fn fetch_body(&self, mailbox: &str, remote_id: &str) -> Result<MessageBody> {
        self.body_on(&self.body, mailbox, remote_id).await
    }

    async fn prefetch_body(&self, mailbox: &str, remote_id: &str) -> Result<MessageBody> {
        self.body_on(&self.prefetch, mailbox, remote_id).await
    }

    async fn set_flags(&self, mailbox: &str, remote_ids: &[String], flags: &Flags) -> Result<()> {
        if remote_ids.is_empty() {
            return Ok(());
        }
        let set = flags.to_imap();
        let unset: Vec<&str> = MANAGED_FLAGS.iter().copied().filter(|f| !set.contains(f)).collect();
        self.with_conn(async |c| {
            c.open(mailbox).await?;
            let s = &mut c.session;
            let uids = uid_set(remote_ids);
            if !set.is_empty() {
                s.uid_store(&uids, format!("+FLAGS.SILENT ({})", set.join(" ")))
                    .await
                    .map_err(cmd)?
                    .try_collect::<Vec<_>>()
                    .await
                    .map_err(cmd)?;
            }
            s.uid_store(&uids, format!("-FLAGS.SILENT ({})", unset.join(" ")))
                .await
                .map_err(cmd)?
                .try_collect::<Vec<_>>()
                .await
                .map_err(cmd)?;
            Ok(())
        })
        .await
    }

    async fn fetch_older(&self, mailbox: &str, since: &SyncState, _: &OlderFrom, count: u32) -> Result<OlderBatch> {
        let unchanged = |more| OlderBatch {
            delta: MailboxDelta { new_state: since.clone(), ..Default::default() },
            more,
        };
        // No cursor yet: the next regular sync sets one.
        let Some(prev) = ImapState::decode(&since.0) else { return Ok(unchanged(true)) };
        if prev.low_uid <= 1 {
            return Ok(unchanged(false));
        }
        self.with_conn(async |c| {
            let mb = c.reselect(mailbox, false).await?;
            let s = &mut c.session;
            if mb.uid_validity.unwrap_or(0) != prev.uid_validity {
                // Renumbered: the next regular sync does the full resync.
                return Ok(unchanged(true));
            }
            let found = s.uid_search(format!("UID 1:{}", prev.low_uid - 1)).await.map_err(cmd)?;
            let (uids, more) = next_older(found, prev.low_uid, count as usize);
            let Some(&low) = uids.first() else { return Ok(unchanged(false)) };
            let set = uids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
            let fetches: Vec<Fetch> = s
                .uid_fetch(set, ENVELOPE_ITEMS)
                .await
                .map_err(cmd)?
                .try_collect()
                .await
                .map_err(cmd)?;
            let st = ImapState { low_uid: low, ..prev };
            Ok(OlderBatch {
                delta: MailboxDelta {
                    added: fetches.iter().filter_map(envelope_of).collect(),
                    new_state: SyncState(st.encode()),
                    ..Default::default()
                },
                more,
            })
        })
        .await
    }

    async fn move_messages(&self, from: &str, remote_ids: &[String], to: &str) -> Result<Vec<String>> {
        if remote_ids.is_empty() {
            return Ok(vec![]);
        }
        self.with_conn(async |c| {
            c.open(from).await?;
            let s = &mut c.session;
            let uids = uid_set(remote_ids);
            if s.uid_mv(&uids, to).await.is_err() {
                // Server without MOVE (RFC 6851): COPY, mark deleted, UID EXPUNGE.
                s.uid_copy(&uids, to).await.map_err(cmd)?;
                s.uid_store(&uids, "+FLAGS.SILENT (\\Deleted)")
                    .await
                    .map_err(cmd)?
                    .try_collect::<Vec<_>>()
                    .await
                    .map_err(cmd)?;
                s.uid_expunge(&uids)
                    .await
                    .map_err(cmd)?
                    .try_collect::<Vec<_>>()
                    .await
                    .map_err(cmd)?;
            }
            // TODO: parse COPYUID from the tagged response to return new UIDs.
            Ok(vec![])
        })
        .await
    }

    async fn append(&self, mailbox: &str, raw: &[u8], flags: &Flags) -> Result<Option<String>> {
        let f = format!("({})", flags.to_imap().join(" "));
        let message_id = header_message_id(raw);
        self.with_conn(async |c| {
            c.session.append(mailbox, Some(&f), None, raw).await.map_err(cmd)?;
            // async-imap doesn't expose APPENDUID, so find the new message by its
            // Message-ID (callers that need the id, like drafts, give each version its own).
            let Some(id) = &message_id else { return Ok(None) };
            // A fresh SELECT, so the search surely sees the new message.
            c.reselect(mailbox, false).await?;
            let s = &mut c.session;
            let found = s
                .uid_search(format!("HEADER Message-ID {}", imap_quote(id)))
                .await
                .map_err(cmd)?;
            Ok(found.into_iter().max().map(|uid| uid.to_string()))
        })
        .await
    }

    async fn expunge(&self, mailbox: &str) -> Result<()> {
        self.with_conn(async |c| {
            c.open(mailbox).await?;
            let s = &mut c.session;
            s.expunge()
                .await
                .map_err(cmd)?
                .try_collect::<Vec<_>>()
                .await
                .map_err(cmd)?;
            Ok(())
        })
        .await
    }

    async fn wait_for_push(&self, mailbox: &str, timeout: Duration) -> Result<Option<PushEvent>> {
        let mut guard = self.idle.lock().await;
        let mut session = match guard.take() {
            Some(s) => s,
            None => connect(&self.server, &self.cred).await?,
        };
        session.select(mailbox).await.map_err(cmd)?;
        let mut idle = session.idle();
        idle.init().await.map_err(net)?;
        let response = {
            let (wait, _stop) = idle.wait_with_timeout(timeout);
            wait.await
        };
        let session = idle.done().await.map_err(net)?;
        *guard = Some(session);
        match response.map_err(net)? {
            IdleResponse::NewData(_) => Ok(Some(PushEvent::MailboxChanged {
                remote_id: mailbox.to_string(),
            })),
            IdleResponse::Timeout | IdleResponse::ManualInterrupt => Ok(None),
        }
    }
}

/// The Message-ID header value (with angle brackets) of a raw message, if it has one.
fn header_message_id(raw: &[u8]) -> Option<String> {
    let end = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or(raw.len().min(16 * 1024));
    let head = String::from_utf8_lossy(&raw[..end]);
    let mut lines = head.split("\r\n").peekable();
    while let Some(line) = lines.next() {
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("message-id")
        {
            // A long id may be folded onto the next line.
            let mut v = value.trim().to_string();
            while lines.peek().is_some_and(|l| l.starts_with([' ', '\t'])) {
                v.push_str(lines.next().unwrap_or_default().trim());
            }
            return Some(v).filter(|v| !v.is_empty());
        }
    }
    None
}

/// An IMAP quoted string.
fn imap_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_message_id_header() {
        let raw = b"From: a@x\r\nMessage-ID: <v2.abc@tern>\r\nSubject: Hi\r\n\r\nbody";
        assert_eq!(header_message_id(raw).as_deref(), Some("<v2.abc@tern>"));
        let folded = b"message-id:\r\n <long.id@host>\r\n\r\n";
        assert_eq!(header_message_id(folded).as_deref(), Some("<long.id@host>"));
        assert_eq!(header_message_id(b"Subject: none\r\n\r\nx"), None);
        assert_eq!(imap_quote("<a\"b@x>"), "\"<a\\\"b@x>\"");
    }

    #[test]
    fn one_mailbox_per_role() {
        let mb = |name: &str, role| RemoteMailbox {
            remote_id: name.into(),
            name: name.into(),
            parent_remote_id: None,
            role,
            selectable: true,
        };
        // Gmail with old labels from a phone: the marked ones win, wherever they are listed.
        let mut boxes = vec![
            mb("Sent", MailboxRole::Sent),
            mb("Sent Messages", MailboxRole::Sent),
            mb("Trash", MailboxRole::Trash),
            mb("[Gmail]/Sent Mail", MailboxRole::Sent),
            mb("[Gmail]/Trash", MailboxRole::Trash),
            mb("Deleted Messages", MailboxRole::Trash),
        ];
        one_per_role(&mut boxes, &[false, false, false, true, true, false]);
        let roles: Vec<_> = boxes.iter().map(|b| (b.remote_id.as_str(), b.role)).collect();
        assert_eq!(
            roles,
            [
                ("Sent", MailboxRole::Custom),
                ("Sent Messages", MailboxRole::Custom),
                ("Trash", MailboxRole::Custom),
                ("[Gmail]/Sent Mail", MailboxRole::Sent),
                ("[Gmail]/Trash", MailboxRole::Trash),
                ("Deleted Messages", MailboxRole::Custom),
            ]
        );

        // No SPECIAL-USE at all: the first name match keeps the role.
        let mut plain = vec![mb("Sent Items", MailboxRole::Sent), mb("Sent", MailboxRole::Sent)];
        one_per_role(&mut plain, &[false, false]);
        assert_eq!((plain[0].role, plain[1].role), (MailboxRole::Sent, MailboxRole::Custom));
    }

    #[test]
    fn roles_from_attributes_and_names() {
        assert_eq!(role_for("INBOX", &[]).0, MailboxRole::Inbox);
        assert_eq!(role_for("[Gmail]/Sent Mail", &[NameAttribute::Sent]), (MailboxRole::Sent, true));
        assert_eq!(role_for("INBOX.Trash", &[]), (MailboxRole::Trash, false));
        assert_eq!(role_for("Deleted Messages", &[]).0, MailboxRole::Trash);
        assert_eq!(role_for("Receipts", &[]).0, MailboxRole::Custom);
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)] // a list of one VANISHED range, on purpose
    fn vanished_ranges_clamp_to_the_cached_window() {
        let uids = |v: Vec<String>| v.into_iter().map(|u| u.parse::<u32>().unwrap()).collect::<Vec<_>>();
        assert_eq!(uids(vanished_in_window(&[5..=5, 7..=7], 1, 10)), [5, 7], "single UIDs");
        assert_eq!(uids(vanished_in_window(&[3..=6], 1, 10)), [3, 4, 5, 6], "a range");
        assert_eq!(uids(vanished_in_window(&[1..=4, 9..=20], 3, 10)), [3, 4, 9, 10], "clamped at both edges");
        assert_eq!(uids(vanished_in_window(&[2..=5, 4..=6], 1, 10)), [2, 3, 4, 5, 6], "overlaps deduplicated");
        assert!(vanished_in_window(&[20..=30], 1, 10).is_empty(), "outside the window");
        assert!(vanished_in_window(&[], 1, 10).is_empty());
    }

    #[test]
    fn sync_mode_uses_the_best_available_tracking() {
        use SyncMode::*;
        let condstore = Extensions { condstore: true, qresync: false };
        assert_eq!(sync_mode(Extensions::ALL, 100, 120, false), Qresync);
        assert_eq!(sync_mode(condstore, 100, 120, false), Condstore);
        assert_eq!(sync_mode(Extensions::NONE, 100, 120, false), Full);
        assert_eq!(sync_mode(Extensions::ALL, 0, 120, false), Full, "v1 cursor: counter unknown");
        assert_eq!(sync_mode(Extensions::ALL, 100, 0, false), Full, "NOMODSEQ mailbox");
        assert_eq!(sync_mode(Extensions::ALL, 100, 90, false), Full, "counter went backwards");
        assert_eq!(sync_mode(Extensions::ALL, 100, 120, true), Full, "periodic full check");
    }

    #[test]
    fn full_check_every_fiftieth_sync() {
        let due: Vec<u32> = (1..=150).filter(|&n| full_check_due(n)).collect();
        assert_eq!(due, [50, 100, 150]);
    }

    #[test]
    fn next_older_picks_the_newest_below_the_window() {
        // Fewer than asked for: all of them, nothing left.
        assert_eq!(next_older([3, 1, 2], 10, 5), (vec![1, 2, 3], false));
        // Exactly the count: all of them, nothing left.
        assert_eq!(next_older([1, 2, 3], 10, 3), (vec![1, 2, 3], false));
        // More than the count: the newest ones, more remain.
        assert_eq!(next_older([1, 5, 2, 4, 3], 10, 2), (vec![4, 5], true));
        // UIDs at or above the window (a sloppy server) are ignored.
        assert_eq!(next_older([2, 10, 11], 10, 5), (vec![2], false));
        // Window already starts at UID 1.
        assert_eq!(next_older([1], 1, 5), (vec![], false));
    }
}
