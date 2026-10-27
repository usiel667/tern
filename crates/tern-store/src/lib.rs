//! Local SQLite cache. Everything the UI shows comes from here, so the app is
//! fully usable offline and opens instantly, as Apple Mail does.
//!
//! `Store` is not `Sync`; the sync engine owns one on its thread and the UI
//! opens its own read connection (SQLite WAL makes that safe).

use std::path::Path;

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};
use serde_json::json;
use tern_core::backend::{MailboxCounts, MailboxDelta, SyncState};
use tern_core::model::*;
use tern_core::search::SearchQuery;
use tern_core::threading::{ThreadInput, group_conversations};
use tern_core::{Error, Result};

const SCHEMA: &str = include_str!("schema.sql");
const SCHEMA_VERSION: i64 = 7;

pub struct Store {
    conn: Connection,
}

fn db(e: rusqlite::Error) -> Error {
    Error::Storage(e.to_string())
}

fn js(e: serde_json::Error) -> Error {
    Error::Storage(e.to_string())
}

/// Columns needed to build a `MessageSummary`, in order.
const SUMMARY_COLS: &str = "m.id, m.account, m.mailbox, m.thread_id, m.remote_id, m.message_id, \
     m.in_reply_to, m.refs, m.subject, m.from_json, m.to_json, m.cc_json, m.date, m.flags_json, \
     m.size, m.has_attachments, m.preview";

/// The same message can sit in several mailboxes of one account: Gmail shows every message in
/// All Mail too, often in Important and Starred, and a message you sent yourself is in Sent and
/// Inbox. Lists keep one copy per Message-ID (messages without one always count), preferring a
/// real mailbox over those catch-alls.
///
/// Only the summary columns go through the window sort: `m.*` would drag every cached body
/// along (1.4 s to list a 1,250-message Inbox once bodies were downloaded ahead).
fn one_copy(filter: &str) -> String {
    format!(
        "SELECT * FROM (
             SELECT {SUMMARY_COLS}, ROW_NUMBER() OVER (
                 PARTITION BY m.account, COALESCE(m.message_id, 'row:' || m.id)
                 ORDER BY CASE b.role WHEN 'all' THEN 2 WHEN 'important' THEN 1 WHEN 'flagged' THEN 1 ELSE 0 END, m.id
             ) AS copy_rank
             FROM messages m JOIN mailboxes b ON b.id = m.mailbox WHERE {filter}
         ) WHERE copy_rank = 1"
    )
}

/// Distinct messages in a conversation (see `one_copy`).
const THREAD_SIZE: &str =
    "(SELECT COUNT(DISTINCT COALESCE(t.message_id, 'row:' || t.id)) FROM messages t WHERE t.thread_id = m.thread_id)";

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Error::Storage(e.to_string()))?;
        }
        Self::init(Connection::open(path).map_err(db)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory().map_err(db)?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA busy_timeout = 5000; PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;",
        )
        .map_err(db)?;
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(db)?;
        if version < SCHEMA_VERSION {
            // Fresh databases get the current shape from SCHEMA; older ones also
            // need the column changes that `CREATE TABLE IF NOT EXISTS` skips.
            conn.execute_batch(SCHEMA).map_err(db)?;
            if version >= 1 {
                migrate(&conn, version)?;
            }
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)
                .map_err(db)?;
        }
        Ok(Self { conn })
    }

    // ---------------------------------------------------------------- mailboxes

    /// Replace the account's folder list with what the server reported. Folders
    /// that disappeared are deleted (cascading to their messages).
    pub fn sync_mailboxes(&mut self, account: AccountId, remote: &[RemoteMailbox]) -> Result<()> {
        let acct = account.to_string();
        let tx = self.conn.transaction().map_err(db)?;
        {
            let mut up = tx
                .prepare(
                    "INSERT INTO mailboxes (account, remote_id, name, parent_remote, role, selectable)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT (account, remote_id) DO UPDATE SET
                       name = excluded.name, parent_remote = excluded.parent_remote,
                       role = excluded.role, selectable = excluded.selectable",
                )
                .map_err(db)?;
            for mb in remote {
                up.execute(params![
                    acct,
                    mb.remote_id,
                    mb.name,
                    mb.parent_remote_id,
                    mb.role.as_str(),
                    mb.selectable
                ])
                .map_err(db)?;
            }
            let keep: Vec<&str> = remote.iter().map(|m| m.remote_id.as_str()).collect();
            let mut existing = tx
                .prepare("SELECT id, remote_id FROM mailboxes WHERE account = ?1")
                .map_err(db)?;
            let gone: Vec<i64> = existing
                .query_map([&acct], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
                .map_err(db)?
                .filter_map(|r| r.ok())
                .filter(|(_, rid)| !keep.contains(&rid.as_str()))
                .map(|(id, _)| id)
                .collect();
            for id in gone {
                tx.execute("DELETE FROM mailboxes WHERE id = ?1", [id])
                    .map_err(db)?;
            }
        }
        tx.commit().map_err(db)
    }

    /// All mailboxes, ordered for the sidebar, with unread/total counts.
    pub fn mailboxes(&self, account: Option<AccountId>) -> Result<Vec<Mailbox>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT b.id, b.account, b.remote_id, b.name, b.parent_remote, b.role, b.selectable,
                        CASE WHEN b.server_unread IS NULL THEN COALESCE(SUM(m.seen = 0), 0)
                             ELSE MAX(0, b.server_unread + COALESCE(SUM(m.seen = 0), 0) - b.base_unread) END,
                        CASE WHEN b.server_total IS NULL THEN COUNT(m.id)
                             ELSE MAX(0, b.server_total + COUNT(m.id) - b.base_total) END
                 FROM mailboxes b LEFT JOIN messages m ON m.mailbox = b.id
                 WHERE (?1 IS NULL OR b.account = ?1)
                 GROUP BY b.id",
            )
            .map_err(db)?;
        let mut out: Vec<Mailbox> = stmt
            .query_map([account.map(|a| a.to_string())], |r| {
                Ok(Mailbox {
                    id: MailboxId(r.get(0)?),
                    account: parse_account(&r.get::<_, String>(1)?),
                    remote: RemoteMailbox {
                        remote_id: r.get(2)?,
                        name: r.get(3)?,
                        parent_remote_id: r.get(4)?,
                        role: MailboxRole::parse(&r.get::<_, String>(5)?),
                        selectable: r.get(6)?,
                    },
                    unread: r.get(7)?,
                    total: r.get(8)?,
                })
            })
            .map_err(db)?
            .collect::<rusqlite::Result<_>>()
            .map_err(db)?;
        out.sort_by(|a, b| {
            (a.account, a.remote.role.sidebar_order(), a.remote.name.to_lowercase()).cmp(&(
                b.account,
                b.remote.role.sidebar_order(),
                b.remote.name.to_lowercase(),
            ))
        });
        Ok(out)
    }

    pub fn mailbox(&self, id: MailboxId) -> Result<Mailbox> {
        self.mailboxes(None)?
            .into_iter()
            .find(|m| m.id == id)
            .ok_or_else(|| Error::NotFound(format!("mailbox {}", id.0)))
    }

    pub fn mailbox_by_role(&self, account: AccountId, role: MailboxRole) -> Result<Option<Mailbox>> {
        Ok(self
            .mailboxes(Some(account))?
            .into_iter()
            .find(|m| m.remote.role == role))
    }

    /// The oldest cached message's remote id and how many are cached (where
    /// "load older" continues from).
    pub fn oldest_cached(&self, mailbox: MailboxId) -> Result<(Option<String>, u32)> {
        let oldest = self
            .conn
            .query_row(
                "SELECT remote_id FROM messages WHERE mailbox = ?1 ORDER BY date, id LIMIT 1",
                [mailbox.0],
                |r| r.get(0),
            )
            .optional()
            .map_err(db)?;
        let cached = self
            .conn
            .query_row("SELECT COUNT(*) FROM messages WHERE mailbox = ?1", [mailbox.0], |r| r.get(0))
            .map_err(db)?;
        Ok((oldest, cached))
    }

    pub fn sync_state(&self, mailbox: MailboxId) -> Result<Option<SyncState>> {
        let s: Option<String> = self
            .conn
            .query_row("SELECT sync_state FROM mailboxes WHERE id = ?1", [mailbox.0], |r| r.get(0))
            .optional()
            .map_err(db)?
            .flatten();
        Ok(s.map(SyncState))
    }

    // ----------------------------------------------------------------- messages

    /// Apply a backend delta atomically, then recompute conversations.
    pub fn apply_delta(&mut self, mailbox: MailboxId, delta: &MailboxDelta) -> Result<()> {
        let (account, role): (String, String) = self
            .conn
            .query_row("SELECT account, role FROM mailboxes WHERE id = ?1", [mailbox.0], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .map_err(db)?;
        let tx = self.conn.transaction().map_err(db)?;
        let cached_before = cached_counts(&tx, mailbox)?;
        {
            // Renumbered (IMAP UIDVALIDITY changed): cached rows may now name other messages.
            // A full resync with the same ids instead updates rows in place below, keeping
            // downloaded bodies, and drops the ones the server no longer lists.
            if delta.full_resync && delta.ids_invalidated {
                tx.execute("DELETE FROM messages WHERE mailbox = ?1", [mailbox.0])
                    .map_err(db)?;
            }
            let mut ins = tx
                .prepare(
                    "INSERT INTO messages (account, mailbox, remote_id, message_id, in_reply_to, refs,
                        subject, from_json, to_json, cc_json, from_text, to_text, date, flags_json,
                        seen, flagged, size, has_attachments, preview)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)
                     ON CONFLICT (mailbox, remote_id) DO UPDATE SET
                        flags_json = excluded.flags_json, seen = excluded.seen,
                        flagged = excluded.flagged,
                        preview = COALESCE(excluded.preview, messages.preview)",
                )
                .map_err(db)?;
            let mut exists = tx
                .prepare("SELECT 1 FROM messages WHERE mailbox = ?1 AND remote_id = ?2")
                .map_err(db)?;
            for e in &delta.added {
                // Learn addresses from new rows only, so flag re-syncs don't inflate counts.
                if !exists.exists(params![mailbox.0, e.remote_id]).map_err(db)? {
                    note_contacts(&tx, &role, &e.from, e.to.iter().chain(&e.cc), e.date.timestamp())?;
                }
                ins.execute(params![
                    account,
                    mailbox.0,
                    e.remote_id,
                    e.message_id,
                    e.in_reply_to,
                    serde_json::to_string(&e.references).map_err(js)?,
                    e.subject,
                    serde_json::to_string(&e.from).map_err(js)?,
                    serde_json::to_string(&e.to).map_err(js)?,
                    serde_json::to_string(&e.cc).map_err(js)?,
                    addr_text(&e.from),
                    addr_text(e.to.iter().chain(&e.cc)),
                    e.date.timestamp(),
                    serde_json::to_string(&e.flags).map_err(js)?,
                    e.flags.seen,
                    e.flags.flag.is_some(),
                    e.size,
                    e.has_attachments,
                    e.preview,
                ])
                .map_err(db)?;
            }
            let mut upd = tx
                .prepare(
                    "UPDATE messages SET flags_json = ?3, seen = ?4, flagged = ?5
                     WHERE mailbox = ?1 AND remote_id = ?2",
                )
                .map_err(db)?;
            for (rid, f) in &delta.flags_changed {
                upd.execute(params![
                    mailbox.0,
                    rid,
                    serde_json::to_string(f).map_err(js)?,
                    f.seen,
                    f.flag.is_some()
                ])
                .map_err(db)?;
            }
            let mut del = tx
                .prepare("DELETE FROM messages WHERE mailbox = ?1 AND remote_id = ?2")
                .map_err(db)?;
            for rid in &delta.removed {
                del.execute(params![mailbox.0, rid]).map_err(db)?;
            }
            let nothing_else: Vec<String> = Vec::new();
            let present = if delta.full_resync { Some(&nothing_else) } else { delta.present.as_ref() };
            if let Some(present) = present {
                let keep: std::collections::HashSet<&str> = present
                    .iter()
                    .chain(delta.added.iter().map(|e| &e.remote_id))
                    .map(String::as_str)
                    .collect();
                let mut all = tx
                    .prepare("SELECT remote_id FROM messages WHERE mailbox = ?1")
                    .map_err(db)?;
                let gone: Vec<String> = all
                    .query_map([mailbox.0], |r| r.get::<_, String>(0))
                    .map_err(db)?
                    .filter_map(|r| r.ok())
                    .filter(|rid| !keep.contains(rid.as_str()))
                    .collect();
                for rid in gone {
                    del.execute(params![mailbox.0, rid]).map_err(db)?;
                }
            }
            tx.execute(
                "UPDATE mailboxes SET sync_state = ?2 WHERE id = ?1",
                params![mailbox.0, delta.new_state.0],
            )
            .map_err(db)?;
            let cached = cached_counts(&tx, mailbox)?;
            match delta.counts {
                // Fresh server counts: they and the cache describe the same moment.
                Some(MailboxCounts { total, unread }) => tx.execute(
                    "UPDATE mailboxes SET server_total = ?2, server_unread = ?3, base_total = ?4, base_unread = ?5
                     WHERE id = ?1",
                    params![mailbox.0, total, unread, cached.total, cached.unread],
                ),
                // No counts (older mail, or the server didn't say): whatever this changed in
                // the cache the server had already counted, so the shown counts stay put.
                None => tx.execute(
                    "UPDATE mailboxes SET base_total = base_total + ?2, base_unread = base_unread + ?3
                     WHERE id = ?1 AND server_total IS NOT NULL",
                    params![
                        mailbox.0,
                        i64::from(cached.total) - i64::from(cached_before.total),
                        i64::from(cached.unread) - i64::from(cached_before.unread)
                    ],
                ),
            }
            .map_err(db)?;
        }
        tx.commit().map_err(db)?;
        self.rethread(parse_account(&account))
    }

    /// Recompute `thread_id` for every message in an account (across mailboxes, so
    /// your Sent replies join the conversation like in Apple Mail).
    ///
    /// TODO(perf): incremental threading for large accounts — only regroup the
    /// components touched by new message-ids.
    pub fn rethread(&mut self, account: AccountId) -> Result<()> {
        struct Row {
            id: i64,
            msgid: Option<String>,
            irt: Option<String>,
            refs: Vec<String>,
            subject: String,
            date: DateTime<Utc>,
        }
        let rows: Vec<Row> = {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT id, message_id, in_reply_to, refs, subject, date FROM messages
                     WHERE account = ?1",
                )
                .map_err(db)?;
            stmt.query_map([account.to_string()], |r| {
                Ok(Row {
                    id: r.get(0)?,
                    msgid: r.get(1)?,
                    irt: r.get(2)?,
                    refs: serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or_default(),
                    subject: r.get(4)?,
                    date: ts(r.get(5)?),
                })
            })
            .map_err(db)?
            .collect::<rusqlite::Result<_>>()
            .map_err(db)?
        };
        let inputs: Vec<ThreadInput> = rows
            .iter()
            .map(|r| ThreadInput {
                message_id: r.msgid.as_deref(),
                in_reply_to: r.irt.as_deref(),
                references: &r.refs,
                subject: &r.subject,
                date: r.date,
            })
            .collect();
        let groups = group_conversations(&inputs);
        let tx = self.conn.transaction().map_err(db)?;
        {
            let mut up = tx
                .prepare("UPDATE messages SET thread_id = ?2 WHERE id = ?1 AND thread_id IS NOT ?2")
                .map_err(db)?;
            for g in groups {
                let tid = g.iter().map(|&i| rows[i].id).min().unwrap();
                for i in g {
                    up.execute(params![rows[i].id, tid]).map_err(db)?;
                }
            }
        }
        tx.commit().map_err(db)
    }

    /// Message list for one mailbox, or a smart mailbox across accounts.
    /// With `conversations`, one row per thread (its newest message in scope)
    /// and `thread_count` set to the whole conversation's size.
    pub fn list(&self, scope: &ListScope, conversations: bool, limit: u32) -> Result<Vec<MessageSummary>> {
        let (filter, args) = scope.sql();
        let sql = if conversations {
            format!(
                "WITH scoped AS ({scoped}),
                 latest AS (SELECT thread_id, MAX(date) AS d FROM scoped GROUP BY COALESCE(thread_id, id))
                 SELECT {cols}, {THREAD_SIZE}
                 FROM scoped m JOIN latest l ON l.thread_id IS m.thread_id AND l.d = m.date
                 GROUP BY COALESCE(m.thread_id, m.id)
                 ORDER BY m.date DESC LIMIT {limit}",
                scoped = one_copy(&filter),
                cols = SUMMARY_COLS
            )
        } else {
            format!(
                "SELECT {cols}, 1 FROM ({scoped}) m ORDER BY m.date DESC LIMIT {limit}",
                scoped = one_copy(&filter),
                cols = SUMMARY_COLS
            )
        };
        let mut stmt = self.conn.prepare(&sql).map_err(db)?;
        stmt.query_map(params_from_iter(args), summary_from_row)
            .map_err(db)?
            .collect::<rusqlite::Result<_>>()
            .map_err(db)
    }

    /// How many messages a scope holds, each counted once (see `one_copy`).
    pub fn count(&self, scope: &ListScope) -> Result<u32> {
        let (filter, args) = scope.sql();
        self.conn
            .query_row(&format!("SELECT COUNT(*) FROM ({})", one_copy(&filter)), params_from_iter(args), |r| r.get(0))
            .map_err(db)
    }

    /// Every message in a conversation, oldest first, across mailboxes (each once).
    pub fn thread(&self, thread_id: i64) -> Result<Vec<MessageSummary>> {
        let sql = format!(
            "SELECT {SUMMARY_COLS}, 1 FROM ({}) m ORDER BY m.date ASC",
            one_copy("m.thread_id = ?1")
        );
        let mut stmt = self.conn.prepare(&sql).map_err(db)?;
        stmt.query_map([thread_id], summary_from_row)
            .map_err(db)?
            .collect::<rusqlite::Result<_>>()
            .map_err(db)
    }

    pub fn message(&self, id: MessageId) -> Result<MessageSummary> {
        let sql = format!("SELECT {SUMMARY_COLS}, 1 FROM messages m WHERE m.id = ?1");
        self.conn
            .query_row(&sql, [id.0], summary_from_row)
            .optional()
            .map_err(db)?
            .ok_or_else(|| Error::NotFound(format!("message {}", id.0)))
    }

    /// Mailbox remote id + message remote id, what a backend needs to act on it.
    pub fn remote_ref(&self, id: MessageId) -> Result<(AccountId, String, String)> {
        self.conn
            .query_row(
                "SELECT m.account, b.remote_id, m.remote_id FROM messages m
                 JOIN mailboxes b ON b.id = m.mailbox WHERE m.id = ?1",
                [id.0],
                |r| Ok((parse_account(&r.get::<_, String>(0)?), r.get(1)?, r.get(2)?)),
            )
            .map_err(db)
    }

    /// The message's body, or that of another copy of it (same Message-ID, same account:
    /// on Gmail a message is in Inbox and All Mail at once) that has one.
    pub fn body(&self, id: MessageId) -> Result<Option<MessageBody>> {
        let row: Option<(Option<String>, Option<String>, Option<String>, Option<Vec<u8>>)> = self
            .conn
            .query_row(
                "SELECT c.body_text, c.body_html, c.body_meta, c.raw FROM messages m JOIN messages c
                   ON c.id = m.id OR (c.account = m.account AND c.message_id = m.message_id)
                 WHERE m.id = ?1 AND c.body_meta IS NOT NULL
                 ORDER BY c.id = m.id DESC LIMIT 1",
                [id.0],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(db)?;
        let Some((text, html, Some(meta), raw)) = row else {
            return Ok(None);
        };
        let meta: serde_json::Value = serde_json::from_str(&meta).map_err(js)?;
        Ok(Some(MessageBody {
            text,
            html,
            attachments: serde_json::from_value(meta["attachments"].clone()).unwrap_or_default(),
            list_unsubscribe: meta["list_unsubscribe"].as_str().map(str::to_owned),
            list_unsubscribe_post: meta["list_unsubscribe_post"].as_bool().unwrap_or(false),
            raw: raw.unwrap_or_default(),
        }))
    }

    /// Messages in `mailbox` worth downloading ahead of time, newest first: no body yet (in
    /// any copy, see `body`) and at most `max_size` bytes.
    pub fn bodies_wanted(&self, mailbox: MailboxId, max_size: u64, limit: usize) -> Result<Vec<MessageId>> {
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT m.id FROM messages m
                 WHERE m.mailbox = ?1 AND m.body_meta IS NULL AND m.size <= ?2
                   AND NOT EXISTS (SELECT 1 FROM messages c WHERE c.message_id = m.message_id
                                   AND c.account = m.account AND c.body_meta IS NOT NULL)
                 ORDER BY m.date DESC LIMIT ?3",
            )
            .map_err(db)?;
        let ids = stmt
            .query_map(params![mailbox.0, max_size as i64, limit as i64], |r| Ok(MessageId(r.get(0)?)))
            .map_err(db)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db)?;
        Ok(ids)
    }

    pub fn store_body(&mut self, id: MessageId, body: &MessageBody) -> Result<()> {
        let meta = json!({
            "attachments": body.attachments,
            "list_unsubscribe": body.list_unsubscribe,
            "list_unsubscribe_post": body.list_unsubscribe_post,
        });
        self.conn
            .execute(
                "UPDATE messages SET body_text = ?2, body_html = ?3, body_meta = ?4, raw = ?5,
                    has_attachments = ?6 WHERE id = ?1",
                params![
                    id.0,
                    body.text,
                    body.html,
                    meta.to_string(),
                    body.raw,
                    body.attachments.iter().any(|a| !a.inline)
                ],
            )
            .map_err(db)?;
        Ok(())
    }

    /// Optimistic local flag change (the engine pushes it to the server).
    pub fn set_flags(&mut self, id: MessageId, flags: &Flags) -> Result<()> {
        self.conn
            .execute(
                "UPDATE messages SET flags_json = ?2, seen = ?3, flagged = ?4 WHERE id = ?1",
                params![id.0, serde_json::to_string(flags).map_err(js)?, flags.seen, flags.flag.is_some()],
            )
            .map_err(db)?;
        Ok(())
    }

    /// `set_flags` keyed by `(mailbox, remote_id)`; a no-op if the row isn't cached.
    pub fn set_flags_remote(&mut self, mailbox: MailboxId, remote_id: &str, flags: &Flags) -> Result<()> {
        self.conn
            .execute(
                "UPDATE messages SET flags_json = ?3, seen = ?4, flagged = ?5
                 WHERE mailbox = ?1 AND remote_id = ?2",
                params![
                    mailbox.0,
                    remote_id,
                    serde_json::to_string(flags).map_err(js)?,
                    flags.seen,
                    flags.flag.is_some()
                ],
            )
            .map_err(db)?;
        Ok(())
    }

    /// `delete_local` keyed by `(mailbox, remote_id)`.
    pub fn delete_remote(&mut self, mailbox: MailboxId, remote_ids: &[String]) -> Result<()> {
        let tx = self.conn.transaction().map_err(db)?;
        for rid in remote_ids {
            tx.execute(
                "DELETE FROM messages WHERE mailbox = ?1 AND remote_id = ?2",
                params![mailbox.0, rid],
            )
            .map_err(db)?;
        }
        tx.commit().map_err(db)
    }

    pub fn delete_local(&mut self, ids: &[MessageId]) -> Result<()> {
        let tx = self.conn.transaction().map_err(db)?;
        for id in ids {
            tx.execute("DELETE FROM messages WHERE id = ?1", [id.0]).map_err(db)?;
        }
        tx.commit().map_err(db)
    }

    // ------------------------------------------------------------------- search

    pub fn search(&self, q: &SearchQuery, scope: &ListScope, limit: u32) -> Result<Vec<MessageSummary>> {
        let (scope_sql, mut args) = scope.sql();
        let mut wheres = vec![scope_sql];
        let fts = fts_expression(q);
        if !fts.is_empty() {
            wheres.push("m.id IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?)".into());
            args.push(fts);
        }
        if let Some(u) = q.unread {
            wheres.push(format!("m.seen = {}", if u { 0 } else { 1 }));
        }
        if let Some(f) = q.flagged {
            wheres.push(format!("m.flagged = {}", f as i32));
        }
        if q.has_attachment == Some(true) {
            wheres.push("m.has_attachments = 1".into());
        }
        if let Some(d) = q.after {
            wheres.push(format!("m.date >= {}", d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp()));
        }
        if let Some(d) = q.before {
            wheres.push(format!("m.date < {}", d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp()));
        }
        let sql = format!(
            "SELECT {SUMMARY_COLS}, 1 FROM ({}) m ORDER BY m.date DESC LIMIT {limit}",
            one_copy(&wheres.join(" AND "))
        );
        let mut stmt = self.conn.prepare(&sql).map_err(db)?;
        stmt.query_map(params_from_iter(args), summary_from_row)
            .map_err(db)?
            .collect::<rusqlite::Result<_>>()
            .map_err(db)
    }

    // ------------------------------------------------------------------- outbox

    /// Queue a message with its SMTP envelope (`to` includes Bcc, which isn't in `raw`).
    pub fn outbox_push(&mut self, account: AccountId, raw: &[u8], from: &str, to: &[String]) -> Result<i64> {
        let envelope = json!({ "from": from, "to": to }).to_string();
        self.conn
            .execute(
                "INSERT INTO outbox (account, raw, envelope, created) VALUES (?1, ?2, ?3, ?4)",
                params![account.to_string(), raw, envelope, Utc::now().timestamp()],
            )
            .map_err(db)?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Messages waiting to be sent (not ones the server refused), oldest first.
    pub fn outbox(&self) -> Result<Vec<OutboxItem>> {
        self.outbox_where("rejected = 0", [])
    }

    /// Messages a server refused for good, oldest first; kept until edited, retried or deleted.
    pub fn outbox_rejected(&self) -> Result<Vec<OutboxItem>> {
        self.outbox_where("rejected = 1", [])
    }

    pub fn outbox_item(&self, id: i64) -> Result<Option<OutboxItem>> {
        Ok(self.outbox_where("id = ?1", [id])?.into_iter().next())
    }

    fn outbox_where<P: rusqlite::Params>(&self, filter: &str, args: P) -> Result<Vec<OutboxItem>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT id, account, raw, envelope, attempts, last_error FROM outbox WHERE {filter} ORDER BY created, id"
            ))
            .map_err(db)?;
        stmt.query_map(args, |r| {
            let envelope = r
                .get::<_, Option<String>>(3)?
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| {
                    let from = v["from"].as_str()?.to_owned();
                    let to = v["to"].as_array()?.iter().filter_map(|a| a.as_str().map(str::to_owned)).collect();
                    Some((from, to))
                });
            Ok(OutboxItem {
                id: r.get(0)?,
                account: parse_account(&r.get::<_, String>(1)?),
                raw: r.get(2)?,
                envelope,
                attempts: r.get(4)?,
                last_error: r.get(5)?,
            })
        })
        .map_err(db)?
        .collect::<rusqlite::Result<_>>()
        .map_err(db)
    }

    /// The server refused this message for good: keep it, but stop sending it.
    pub fn outbox_reject(&mut self, id: i64, err: &str) -> Result<()> {
        self.conn
            .execute(
                "UPDATE outbox SET rejected = 1, attempts = attempts + 1, last_error = ?2 WHERE id = ?1",
                params![id, err],
            )
            .map_err(db)?;
        Ok(())
    }

    /// Put a refused message back in line to be sent.
    pub fn outbox_retry(&mut self, id: i64) -> Result<()> {
        self.conn.execute("UPDATE outbox SET rejected = 0 WHERE id = ?1", [id]).map_err(db)?;
        Ok(())
    }

    pub fn outbox_done(&mut self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM outbox WHERE id = ?1", [id]).map_err(db)?;
        Ok(())
    }

    // -------------------------------------------------------------- pending ops
    // Queued message actions. `op` is opaque JSON owned by the sync engine.

    pub fn pending_push(&mut self, account: AccountId, op: &str) -> Result<i64> {
        self.conn
            .execute(
                "INSERT INTO pending_ops (account, op, created) VALUES (?1, ?2, ?3)",
                params![account.to_string(), op, Utc::now().timestamp()],
            )
            .map_err(db)?;
        Ok(self.conn.last_insert_rowid())
    }

    /// An account's queued ops, oldest first, as `(id, op)`.
    pub fn pending_ops(&self, account: AccountId) -> Result<Vec<(i64, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, op FROM pending_ops WHERE account = ?1 ORDER BY id")
            .map_err(db)?;
        stmt.query_map([account.to_string()], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(db)?
            .collect::<rusqlite::Result<_>>()
            .map_err(db)
    }

    pub fn pending_done(&mut self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM pending_ops WHERE id = ?1", [id]).map_err(db)?;
        Ok(())
    }

    pub fn pending_clear(&mut self, account: AccountId) -> Result<()> {
        self.conn
            .execute("DELETE FROM pending_ops WHERE account = ?1", [account.to_string()])
            .map_err(db)?;
        Ok(())
    }

    pub fn outbox_failed(&mut self, id: i64, err: &str) -> Result<()> {
        self.conn
            .execute(
                "UPDATE outbox SET attempts = attempts + 1, last_error = ?2 WHERE id = ?1",
                params![id, err],
            )
            .map_err(db)?;
        Ok(())
    }

    // ------------------------------------------------------------------- drafts

    /// Save a new version of a draft (creating it if needed). It's marked not uploaded.
    pub fn draft_put(&mut self, key: &str, account: AccountId, raw: &[u8], message_id: Option<&str>) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO drafts (key, account, raw, message_id) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (key) DO UPDATE SET raw = excluded.raw, message_id = excluded.message_id,
                    account = excluded.account, version = version + 1, uploaded = 0",
                params![key, account.to_string(), raw, message_id],
            )
            .map_err(db)?;
        Ok(())
    }

    pub fn draft(&self, key: &str) -> Result<Option<DraftRow>> {
        Ok(self.drafts_where("key = ?1", [key])?.into_iter().next())
    }

    /// Drafts whose latest version isn't on the server yet.
    pub fn drafts_pending(&self) -> Result<Vec<DraftRow>> {
        self.drafts_where("uploaded = 0", [])
    }

    fn drafts_where<P: rusqlite::Params>(&self, filter: &str, args: P) -> Result<Vec<DraftRow>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT key, account, raw, message_id, version, uploaded, remote_mailbox, remote_id, remote_msgid
                 FROM drafts WHERE {filter} ORDER BY rowid"
            ))
            .map_err(db)?;
        stmt.query_map(args, |r| {
            Ok(DraftRow {
                key: r.get(0)?,
                account: parse_account(&r.get::<_, String>(1)?),
                raw: r.get(2)?,
                message_id: r.get(3)?,
                version: r.get(4)?,
                uploaded: r.get(5)?,
                remote: match r.get::<_, Option<i64>>(6)? {
                    Some(m) => Some(ServerCopy {
                        mailbox: MailboxId(m),
                        remote_id: r.get(7)?,
                        message_id: r.get(8)?,
                    }),
                    None => None,
                },
            })
        })
        .map_err(db)?
        .collect::<rusqlite::Result<_>>()
        .map_err(db)
    }

    /// Record where the draft's server copy now is. `version` is the version that was
    /// uploaded: if the draft changed meanwhile, it stays marked not uploaded.
    pub fn draft_uploaded(&mut self, key: &str, copy: &ServerCopy, version: i64) -> Result<()> {
        self.conn
            .execute(
                "UPDATE drafts SET remote_mailbox = ?2, remote_id = ?3, remote_msgid = ?4, uploaded = (version = ?5)
                 WHERE key = ?1",
                params![key, copy.mailbox.0, copy.remote_id, copy.message_id, version],
            )
            .map_err(db)?;
        Ok(())
    }

    /// Forget other drafts whose server copy is `copy` and that have nothing newer to
    /// upload (a window closed after saving, now reopened under a new key).
    pub fn draft_forget_others(&mut self, key: &str, copy: &ServerCopy) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM drafts WHERE key <> ?1 AND uploaded = 1 AND remote_mailbox = ?2
                   AND (remote_id = ?3 OR (remote_id IS NULL AND remote_msgid = ?4))",
                params![key, copy.mailbox.0, copy.remote_id, copy.message_id],
            )
            .map_err(db)?;
        Ok(())
    }

    pub fn draft_delete(&mut self, key: &str) -> Result<()> {
        self.conn.execute("DELETE FROM drafts WHERE key = ?1", [key]).map_err(db)?;
        Ok(())
    }

    /// A cached message in `mailbox` with this Message-ID, as `(id, remote id)`.
    pub fn find_by_message_id(&self, mailbox: MailboxId, message_id: &str) -> Result<Option<(MessageId, String)>> {
        self.conn
            .query_row(
                "SELECT id, remote_id FROM messages WHERE mailbox = ?1 AND message_id = ?2 ORDER BY id DESC LIMIT 1",
                params![mailbox.0, message_id],
                |r| Ok((MessageId(r.get(0)?), r.get(1)?)),
            )
            .optional()
            .map_err(db)
    }

    // ----------------------------------------------------------------- contacts

    /// Addresses whose email, or any word of whose name, starts with `prefix`; people the
    /// user has written to first, then by how often and how recently they were seen.
    pub fn suggest_addresses(&self, prefix: &str, limit: usize) -> Result<Vec<Address>> {
        let prefix = prefix.trim();
        if prefix.is_empty() {
            return Ok(Vec::new());
        }
        let like = prefix.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT email, name FROM contacts
                 WHERE email LIKE ?1 || '%' ESCAPE '\\'
                    OR name LIKE ?1 || '%' ESCAPE '\\'
                    OR name LIKE '% ' || ?1 || '%' ESCAPE '\\'
                 ORDER BY sent > 0 DESC, sent * 4 + received DESC, last_seen DESC
                 LIMIT ?2",
            )
            .map_err(db)?;
        let rows = stmt
            .query_map(params![like, limit as i64], |r| {
                Ok(Address { email: r.get(0)?, name: r.get(1)? })
            })
            .map_err(db)?;
        rows.collect::<rusqlite::Result<_>>().map_err(db)
    }
}

/// Upgrade an existing database from `from` to `SCHEMA_VERSION`, one step at a time.
fn migrate(conn: &Connection, from: i64) -> Result<()> {
    if from < 2 {
        add_column(conn, "outbox", "envelope", "TEXT")?;
    }
    if from < 3 {
        backfill_contacts(conn)?;
    }
    if from < 4 {
        add_column(conn, "outbox", "rejected", "INTEGER NOT NULL DEFAULT 0")?;
    }
    // v5 only adds the `drafts` table, which SCHEMA creates.
    if from < 6 {
        for column in ["server_total", "server_unread", "base_total", "base_unread"] {
            add_column(conn, "mailboxes", column, "INTEGER")?;
        }
    }
    if from < 7 {
        // Previews of multipart mail were stored undecoded (base64 / quoted-printable): fetch
        // every mailbox's list again. Rows are updated in place, so bodies stay.
        conn.execute("UPDATE mailboxes SET sync_state = NULL", []).map_err(db)?;
    }
    Ok(())
}

/// How many messages of a mailbox are cached, and how many of those are unread.
fn cached_counts(conn: &Connection, mailbox: MailboxId) -> Result<MailboxCounts> {
    conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(seen = 0), 0) FROM messages WHERE mailbox = ?1",
        [mailbox.0],
        |r| Ok(MailboxCounts { total: r.get(0)?, unread: r.get(1)? }),
    )
    .map_err(db)
}

/// `ALTER TABLE … ADD COLUMN` unless the column is already there (a half-finished upgrade
/// shouldn't make every later start fail).
fn add_column(conn: &Connection, table: &str, column: &str, decl: &str) -> Result<()> {
    let exists: bool = conn
        .query_row(
            &format!("SELECT COUNT(*) > 0 FROM pragma_table_info('{table}') WHERE name = ?1"),
            [column],
            |r| r.get(0),
        )
        .map_err(db)?;
    if !exists {
        conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl};")).map_err(db)?;
    }
    Ok(())
}

/// Learn the addresses of mail cached before the `contacts` table existed.
fn backfill_contacts(conn: &Connection) -> Result<()> {
    conn.execute_batch("BEGIN").map_err(db)?;
    let result = (|| {
        let mut stmt = conn
            .prepare(
                "SELECT b.role, m.from_json, m.to_json, m.cc_json, m.date
                 FROM messages m JOIN mailboxes b ON b.id = m.mailbox",
            )
            .map_err(db)?;
        let rows = stmt
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?, r.get::<_, i64>(4)?))
            })
            .map_err(db)?;
        for row in rows {
            let (role, from, to, cc, date) = row.map_err(db)?;
            let parse = |j: &str| serde_json::from_str::<Vec<Address>>(j).unwrap_or_default();
            let (from, to, cc) = (parse(&from), parse(&to), parse(&cc));
            note_contacts(conn, &role, &from, to.iter().chain(&cc), date)?;
        }
        Ok(())
    })();
    conn.execute_batch(if result.is_ok() { "COMMIT" } else { "ROLLBACK" }).map_err(db)?;
    result
}

/// Record the people in a newly stored message: recipients of mail in Sent are people the
/// user wrote to, senders of mail elsewhere wrote to the user. Drafts, Junk and Trash teach
/// nothing, so spam and unsent mail never become suggestions.
fn note_contacts<'a>(
    conn: &Connection,
    role: &str,
    from: &'a [Address],
    recipients: impl Iterator<Item = &'a Address>,
    date: i64,
) -> Result<()> {
    let (people, sent): (Vec<&Address>, bool) = match role {
        "sent" => (recipients.collect(), true),
        "drafts" | "junk" | "trash" => return Ok(()),
        _ => (from.iter().collect(), false),
    };
    let mut up = conn
        .prepare_cached(
            "INSERT INTO contacts (email, name, sent, received, last_seen) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (email) DO UPDATE SET
               sent = sent + excluded.sent,
               received = received + excluded.received,
               name = CASE WHEN excluded.name IS NOT NULL AND excluded.last_seen >= last_seen
                           THEN excluded.name ELSE COALESCE(name, excluded.name) END,
               last_seen = MAX(last_seen, excluded.last_seen)",
        )
        .map_err(db)?;
    for a in people {
        let email = a.email.trim();
        if !email.contains('@') || is_machine_address(email) {
            continue;
        }
        let name = a.name.as_deref().map(str::trim).filter(|n| !n.is_empty() && *n != email);
        up.execute(params![email, name, sent as i64, !sent as i64, date])
            .map_err(db)?;
    }
    Ok(())
}

/// Automated senders nobody writes to.
fn is_machine_address(email: &str) -> bool {
    let local = email.split('@').next().unwrap_or("").to_ascii_lowercase();
    ["noreply", "no-reply", "no_reply", "donotreply", "do-not-reply", "do_not_reply", "mailer-daemon", "postmaster"]
        .contains(&local.as_str())
        || local.starts_with("bounce")
        || local.starts_with("noreply")
        || local.starts_with("no-reply")
}

/// A message being written, as last saved.
#[derive(Debug, Clone)]
pub struct DraftRow {
    pub key: String,
    pub account: AccountId,
    pub raw: Vec<u8>,
    pub message_id: Option<String>,
    pub version: i64,
    /// The server copy is the latest version.
    pub uploaded: bool,
    /// Where the server copy is, if there is one.
    pub remote: Option<ServerCopy>,
}

/// A draft's copy in a Drafts mailbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerCopy {
    pub mailbox: MailboxId,
    /// Known when the server reported it (JMAP, or IMAP after a Message-ID search).
    pub remote_id: Option<String>,
    pub message_id: Option<String>,
}

/// A queued send.
#[derive(Debug, Clone)]
pub struct OutboxItem {
    pub id: i64,
    pub account: AccountId,
    pub raw: Vec<u8>,
    /// SMTP envelope `(from, recipients)`, Bcc included. `None` for rows queued
    /// before schema v2; those fall back to the headers and lose Bcc.
    pub envelope: Option<(String, Vec<String>)>,
    pub attempts: u32,
    pub last_error: Option<String>,
}

/// What the message list is showing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListScope {
    Mailbox(MailboxId),
    /// Smart mailbox across all accounts: "All Inboxes", "All Sent", ...
    Role(MailboxRole),
    /// Apple Mail's "Flagged" favourite.
    Flagged,
    /// "Unread" smart mailbox.
    Unread,
    Everything,
}

impl ListScope {
    fn sql(&self) -> (String, Vec<String>) {
        match self {
            Self::Mailbox(id) => ("m.mailbox = ?".into(), vec![id.0.to_string()]),
            Self::Role(role) => ("b.role = ?".into(), vec![role.as_str().into()]),
            Self::Flagged => ("m.flagged = 1 AND b.role NOT IN ('trash','junk')".into(), vec![]),
            Self::Unread => ("m.seen = 0 AND b.role NOT IN ('trash','junk')".into(), vec![]),
            Self::Everything => ("1 = 1".into(), vec![]),
        }
    }
}

fn fts_expression(q: &SearchQuery) -> String {
    fn quote(s: &str) -> String {
        format!("\"{}\"*", s.replace('"', "\"\""))
    }
    let mut parts = Vec::new();
    for t in &q.text {
        parts.push(quote(t));
    }
    for t in &q.from {
        parts.push(format!("from_text : {}", quote(t)));
    }
    for t in &q.to {
        parts.push(format!("to_text : {}", quote(t)));
    }
    for t in &q.subject {
        parts.push(format!("subject : {}", quote(t)));
    }
    parts.join(" AND ")
}

fn addr_text<'a>(addrs: impl IntoIterator<Item = &'a Address>) -> String {
    addrs
        .into_iter()
        .map(|a| match &a.name {
            Some(n) => format!("{n} {}", a.email),
            None => a.email.clone(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn ts(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(secs, 0).single().unwrap_or_default()
}

fn parse_account(s: &str) -> AccountId {
    AccountId(s.parse().unwrap_or_default())
}

fn summary_from_row(r: &Row<'_>) -> rusqlite::Result<MessageSummary> {
    let jsonv = |i: usize| -> rusqlite::Result<String> { r.get(i) };
    Ok(MessageSummary {
        id: MessageId(r.get(0)?),
        account: parse_account(&r.get::<_, String>(1)?),
        mailbox: MailboxId(r.get(2)?),
        thread_id: r.get(3)?,
        envelope: Envelope {
            remote_id: r.get(4)?,
            message_id: r.get(5)?,
            in_reply_to: r.get(6)?,
            references: serde_json::from_str(&jsonv(7)?).unwrap_or_default(),
            subject: r.get(8)?,
            from: serde_json::from_str(&jsonv(9)?).unwrap_or_default(),
            to: serde_json::from_str(&jsonv(10)?).unwrap_or_default(),
            cc: serde_json::from_str(&jsonv(11)?).unwrap_or_default(),
            date: ts(r.get(12)?),
            flags: serde_json::from_str(&jsonv(13)?).unwrap_or_default(),
            size: r.get(14)?,
            has_attachments: r.get(15)?,
            preview: r.get(16)?,
        },
        thread_count: r.get(17)?,
    })
}

#[cfg(test)]
mod tests;
