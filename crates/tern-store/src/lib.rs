//! Local SQLite cache. Everything the UI shows comes from here, so the app is
//! fully usable offline and opens instantly, as Apple Mail does.
//!
//! `Store` is not `Sync`; the sync engine owns one on its thread and the UI
//! opens its own read connection (SQLite WAL makes that safe).

use std::path::Path;

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};
use serde_json::json;
use tern_core::backend::{MailboxDelta, SyncState};
use tern_core::model::*;
use tern_core::search::SearchQuery;
use tern_core::threading::{ThreadInput, group_conversations};
use tern_core::{Error, Result};

const SCHEMA: &str = include_str!("schema.sql");
const SCHEMA_VERSION: i64 = 1;

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
            conn.execute_batch(SCHEMA).map_err(db)?;
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
                        COALESCE(SUM(m.seen = 0), 0), COUNT(m.id)
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
        let account: String = self
            .conn
            .query_row("SELECT account FROM mailboxes WHERE id = ?1", [mailbox.0], |r| r.get(0))
            .map_err(db)?;
        let tx = self.conn.transaction().map_err(db)?;
        {
            if delta.full_resync {
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
            for e in &delta.added {
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
            if let Some(present) = &delta.present {
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
                "WITH scoped AS (SELECT m.* FROM messages m JOIN mailboxes b ON b.id = m.mailbox WHERE {filter}),
                 latest AS (SELECT thread_id, MAX(date) AS d FROM scoped GROUP BY COALESCE(thread_id, id))
                 SELECT {cols}, (SELECT COUNT(*) FROM messages t WHERE t.thread_id = m.thread_id)
                 FROM scoped m JOIN latest l ON l.thread_id IS m.thread_id AND l.d = m.date
                 GROUP BY COALESCE(m.thread_id, m.id)
                 ORDER BY m.date DESC LIMIT {limit}",
                cols = SUMMARY_COLS
            )
        } else {
            format!(
                "SELECT {cols}, 1 FROM messages m JOIN mailboxes b ON b.id = m.mailbox
                 WHERE {filter} ORDER BY m.date DESC LIMIT {limit}",
                cols = SUMMARY_COLS
            )
        };
        let mut stmt = self.conn.prepare(&sql).map_err(db)?;
        stmt.query_map(params_from_iter(args), summary_from_row)
            .map_err(db)?
            .collect::<rusqlite::Result<_>>()
            .map_err(db)
    }

    /// Every message in a conversation, oldest first, across mailboxes.
    pub fn thread(&self, thread_id: i64) -> Result<Vec<MessageSummary>> {
        let sql = format!(
            "SELECT {SUMMARY_COLS}, 1 FROM messages m WHERE m.thread_id = ?1 ORDER BY m.date ASC"
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

    pub fn body(&self, id: MessageId) -> Result<Option<MessageBody>> {
        let row: Option<(Option<String>, Option<String>, Option<String>, Option<Vec<u8>>)> = self
            .conn
            .query_row(
                "SELECT body_text, body_html, body_meta, raw FROM messages WHERE id = ?1",
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
            "SELECT {SUMMARY_COLS}, 1 FROM messages m JOIN mailboxes b ON b.id = m.mailbox
             WHERE {} ORDER BY m.date DESC LIMIT {limit}",
            wheres.join(" AND ")
        );
        let mut stmt = self.conn.prepare(&sql).map_err(db)?;
        stmt.query_map(params_from_iter(args), summary_from_row)
            .map_err(db)?
            .collect::<rusqlite::Result<_>>()
            .map_err(db)
    }

    // ------------------------------------------------------------------- outbox

    pub fn outbox_push(&mut self, account: AccountId, raw: &[u8]) -> Result<i64> {
        self.conn
            .execute(
                "INSERT INTO outbox (account, raw, created) VALUES (?1, ?2, ?3)",
                params![account.to_string(), raw, Utc::now().timestamp()],
            )
            .map_err(db)?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn outbox(&self) -> Result<Vec<(i64, AccountId, Vec<u8>)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, account, raw FROM outbox ORDER BY created")
            .map_err(db)?;
        stmt.query_map([], |r| Ok((r.get(0)?, parse_account(&r.get::<_, String>(1)?), r.get(2)?)))
            .map_err(db)?
            .collect::<rusqlite::Result<_>>()
            .map_err(db)
    }

    pub fn outbox_done(&mut self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM outbox WHERE id = ?1", [id]).map_err(db)?;
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
