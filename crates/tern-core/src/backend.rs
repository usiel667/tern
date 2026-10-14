//! The protocol-neutral interface every account backend implements.
//!
//! The sync engine only talks to `dyn MailBackend`, so IMAP and JMAP accounts
//! look identical to the store and the UI.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::model::{Envelope, Flags, MessageBody, RemoteMailbox};

/// Opaque per-mailbox sync cursor. For IMAP this encodes UIDVALIDITY, the
/// highest UID seen and HIGHESTMODSEQ (CONDSTORE); for JMAP it is the
/// `Email/changes` state string. Backends own the format.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncState(pub String);

/// What changed in a mailbox since a [`SyncState`].
#[derive(Debug, Clone, Default)]
pub struct MailboxDelta {
    /// New messages (or all messages on a full resync).
    pub added: Vec<Envelope>,
    /// Flag changes on messages we already have, by remote id.
    pub flags_changed: Vec<(String, Flags)>,
    /// Remote ids that no longer exist in this mailbox.
    pub removed: Vec<String>,
    /// When `Some`, the complete set of remote ids that still exist among those
    /// the client could already know about; any cached message not in this set
    /// (and not in `added`) is gone. IMAP without QRESYNC uses this.
    pub present: Option<Vec<String>>,
    /// True when the cursor was invalid (e.g. UIDVALIDITY changed) and the
    /// caller must drop every cached message for this mailbox first.
    pub full_resync: bool,
    pub new_state: SyncState,
}

/// Something happened on the server that the engine should react to.
#[derive(Debug, Clone)]
pub enum PushEvent {
    MailboxChanged { remote_id: String },
    Disconnected,
}

#[async_trait]
pub trait MailBackend: Send + Sync {
    /// Short name for logs ("imap", "jmap").
    fn kind(&self) -> &'static str;

    async fn list_mailboxes(&self) -> Result<Vec<RemoteMailbox>>;

    /// Incremental fetch. `since = None` means initial sync; backends may cap
    /// how far back they go (`limit`) to keep first launch fast, like Apple Mail.
    async fn sync_mailbox(
        &self,
        mailbox: &str,
        since: Option<&SyncState>,
        limit: Option<u32>,
    ) -> Result<MailboxDelta>;

    async fn fetch_body(&self, mailbox: &str, remote_id: &str) -> Result<MessageBody>;

    async fn set_flags(&self, mailbox: &str, remote_ids: &[String], flags: &Flags) -> Result<()>;

    /// Move messages; returns new remote ids in the destination when the server reports them.
    async fn move_messages(
        &self,
        from: &str,
        remote_ids: &[String],
        to: &str,
    ) -> Result<Vec<String>>;

    /// Store a raw RFC 5322 message (drafts, sent copies). Returns its remote id if known.
    async fn append(&self, mailbox: &str, raw: &[u8], flags: &Flags) -> Result<Option<String>>;

    /// Permanently remove messages flagged `\Deleted` (IMAP EXPUNGE). No-op for JMAP.
    async fn expunge(&self, mailbox: &str) -> Result<()>;

    /// Block until the server pushes a change (IMAP IDLE / JMAP EventSource) or
    /// the timeout passes. Returns `None` on timeout.
    async fn wait_for_push(
        &self,
        mailbox: &str,
        timeout: std::time::Duration,
    ) -> Result<Option<PushEvent>>;

    /// Whether this backend submits mail itself (JMAP) rather than via SMTP.
    fn handles_submission(&self) -> bool {
        false
    }

    /// Send a fully built message (only when [`Self::handles_submission`]).
    /// The backend also files the copy in Sent.
    async fn submit(&self, _raw: &[u8], _from: &str, _recipients: &[String]) -> Result<()> {
        Err(crate::Error::Unsupported("submission via this backend"))
    }
}
