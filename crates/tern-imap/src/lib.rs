//! IMAP4rev1 backend.
//!
//! Two connections per account, as most clients do: one for commands, one that
//! sits in IDLE on the inbox for push. Sync is UID-based and works on any
//! server; CONDSTORE/QRESYNC are an optimisation to add later (see md/PLAN.md).

mod connect;
mod state;

use std::time::Duration;

use async_imap::extensions::idle::IdleResponse;
use async_imap::types::{Fetch, Flag, NameAttribute};
use async_trait::async_trait;
use futures::TryStreamExt;
use tern_auth::Credential;
use tern_core::account::ServerConfig;
use tern_core::backend::{MailBackend, MailboxDelta, OlderBatch, OlderFrom, PushEvent, SyncState};
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

pub struct ImapBackend {
    server: ServerConfig,
    cred: Credential,
    cmd: Mutex<Option<Session>>,
    idle: Mutex<Option<Session>>,
}

impl ImapBackend {
    pub fn new(server: ServerConfig, cred: Credential) -> Self {
        Self {
            server,
            cred,
            cmd: Mutex::new(None),
            idle: Mutex::new(None),
        }
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
        let mut guard = self.cmd.lock().await;
        if guard.is_none() {
            *guard = Some(connect(&self.server, &self.cred).await?);
        }
        let result = f(guard.as_mut().expect("just connected")).await;
        if matches!(result, Err(Error::Network(_) | Error::Protocol(_))) {
            *guard = None;
        }
        result
    }
}

fn role_for(name: &str, attrs: &[NameAttribute<'_>]) -> MailboxRole {
    if name.eq_ignore_ascii_case("INBOX") {
        return MailboxRole::Inbox;
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
        return role;
    }
    // Servers without SPECIAL-USE: guess from common names, as Apple Mail does.
    let leaf = name.rsplit(['/', '.']).next().unwrap_or(name).to_ascii_lowercase();
    match leaf.as_str() {
        "drafts" => MailboxRole::Drafts,
        "sent" | "sent items" | "sent messages" | "sent mail" => MailboxRole::Sent,
        "junk" | "spam" | "junk e-mail" => MailboxRole::Junk,
        "trash" | "deleted items" | "deleted messages" | "bin" => MailboxRole::Trash,
        "archive" | "archives" => MailboxRole::Archive,
        _ => MailboxRole::Custom,
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
            Ok(names
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
                    RemoteMailbox {
                        role: role_for(&full, n.attributes()),
                        name: if full.eq_ignore_ascii_case("INBOX") { "Inbox".into() } else { leaf },
                        remote_id: full,
                        parent_remote_id: parent,
                        selectable,
                    }
                })
                .collect())
        })
        .await
    }

    async fn sync_mailbox(
        &self,
        mailbox: &str,
        since: Option<&SyncState>,
        limit: Option<u32>,
    ) -> Result<MailboxDelta> {
        self.with_session(async |s| {
            let mb = s.select(mailbox).await.map_err(cmd)?;
            let validity = mb.uid_validity.unwrap_or(0);
            let prev = since.and_then(|st| ImapState::decode(&st.0));
            let mut delta = MailboxDelta {
                ids_invalidated: prev.as_ref().is_some_and(|p| p.uid_validity != validity),
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
                    };
                    delta.added = added;
                    delta.new_state = SyncState(st.encode());
                }
                Some(p) => {
                    let mut st = p.clone();
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
                        let fetches: Vec<Fetch> = s
                            .uid_fetch(format!("{}:{}", p.low_uid, p.high_uid), "(UID FLAGS)")
                            .await
                            .map_err(cmd)?
                            .try_collect()
                            .await
                            .map_err(cmd)?;
                        let mut present = Vec::with_capacity(fetches.len());
                        for f in &fetches {
                            if let Some(uid) = f.uid {
                                present.push(uid.to_string());
                                delta.flags_changed.push((uid.to_string(), flags_of(f)));
                            }
                        }
                        delta.present = Some(present);
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
        self.with_session(async |s| {
            s.select(mailbox).await.map_err(cmd)?;
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

    async fn set_flags(&self, mailbox: &str, remote_ids: &[String], flags: &Flags) -> Result<()> {
        if remote_ids.is_empty() {
            return Ok(());
        }
        let set = flags.to_imap();
        let unset: Vec<&str> = MANAGED_FLAGS.iter().copied().filter(|f| !set.contains(f)).collect();
        self.with_session(async |s| {
            s.select(mailbox).await.map_err(cmd)?;
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
        self.with_session(async |s| {
            let mb = s.select(mailbox).await.map_err(cmd)?;
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
        self.with_session(async |s| {
            s.select(from).await.map_err(cmd)?;
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
        self.with_session(async |s| {
            s.append(mailbox, Some(&f), None, raw).await.map_err(cmd)?;
            Ok(None)
        })
        .await
    }

    async fn expunge(&self, mailbox: &str) -> Result<()> {
        self.with_session(async |s| {
            s.select(mailbox).await.map_err(cmd)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_from_attributes_and_names() {
        assert_eq!(role_for("INBOX", &[]), MailboxRole::Inbox);
        assert_eq!(role_for("[Gmail]/Sent Mail", &[NameAttribute::Sent]), MailboxRole::Sent);
        assert_eq!(role_for("INBOX.Trash", &[]), MailboxRole::Trash);
        assert_eq!(role_for("Deleted Messages", &[]), MailboxRole::Trash);
        assert_eq!(role_for("Receipts", &[]), MailboxRole::Custom);
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
