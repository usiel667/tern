-- Tern local cache. Bump `user_version` and add a migration in lib.rs when changing.

CREATE TABLE IF NOT EXISTS mailboxes (
    id            INTEGER PRIMARY KEY,
    account       TEXT    NOT NULL,
    remote_id     TEXT    NOT NULL,
    name          TEXT    NOT NULL,
    parent_remote TEXT,
    role          TEXT    NOT NULL,
    selectable    INTEGER NOT NULL DEFAULT 1,
    sync_state    TEXT,
    UNIQUE (account, remote_id)
);

CREATE TABLE IF NOT EXISTS messages (
    id              INTEGER PRIMARY KEY,
    account         TEXT    NOT NULL,
    mailbox         INTEGER NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE,
    remote_id       TEXT    NOT NULL,
    message_id      TEXT,
    in_reply_to     TEXT,
    refs            TEXT    NOT NULL DEFAULT '[]',   -- JSON array
    subject         TEXT    NOT NULL DEFAULT '',
    from_json       TEXT    NOT NULL DEFAULT '[]',
    to_json         TEXT    NOT NULL DEFAULT '[]',
    cc_json         TEXT    NOT NULL DEFAULT '[]',
    from_text       TEXT    NOT NULL DEFAULT '',      -- denormalised for FTS
    to_text         TEXT    NOT NULL DEFAULT '',
    date            INTEGER NOT NULL,                 -- unix seconds
    flags_json      TEXT    NOT NULL,
    seen            INTEGER NOT NULL DEFAULT 0,
    flagged         INTEGER NOT NULL DEFAULT 0,
    size            INTEGER NOT NULL DEFAULT 0,
    has_attachments INTEGER NOT NULL DEFAULT 0,
    preview         TEXT,
    thread_id       INTEGER,
    body_text       TEXT,
    body_html       TEXT,
    body_meta       TEXT,                             -- JSON: attachments, unsubscribe
    raw             BLOB,
    UNIQUE (mailbox, remote_id)
);

CREATE INDEX IF NOT EXISTS messages_mailbox_date ON messages (mailbox, date DESC);
CREATE INDEX IF NOT EXISTS messages_thread ON messages (thread_id);
CREATE INDEX IF NOT EXISTS messages_msgid ON messages (message_id);
CREATE INDEX IF NOT EXISTS messages_unread ON messages (seen) WHERE seen = 0;
CREATE INDEX IF NOT EXISTS messages_flagged ON messages (flagged) WHERE flagged = 1;

-- External-content FTS index kept in sync by triggers.
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5 (
    subject, from_text, to_text, preview, body_text,
    content = 'messages', content_rowid = 'id',
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts (rowid, subject, from_text, to_text, preview, body_text)
    VALUES (new.id, new.subject, new.from_text, new.to_text, new.preview, new.body_text);
END;
CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
    INSERT INTO messages_fts (messages_fts, rowid, subject, from_text, to_text, preview, body_text)
    VALUES ('delete', old.id, old.subject, old.from_text, old.to_text, old.preview, old.body_text);
END;
CREATE TRIGGER IF NOT EXISTS messages_au AFTER UPDATE OF subject, from_text, to_text, preview, body_text ON messages BEGIN
    INSERT INTO messages_fts (messages_fts, rowid, subject, from_text, to_text, preview, body_text)
    VALUES ('delete', old.id, old.subject, old.from_text, old.to_text, old.preview, old.body_text);
    INSERT INTO messages_fts (rowid, subject, from_text, to_text, preview, body_text)
    VALUES (new.id, new.subject, new.from_text, new.to_text, new.preview, new.body_text);
END;

-- Outgoing mail that hasn't been accepted by the server yet (offline send).
CREATE TABLE IF NOT EXISTS outbox (
    id         INTEGER PRIMARY KEY,
    account    TEXT    NOT NULL,
    raw        BLOB    NOT NULL,
    envelope   TEXT,              -- JSON {"from", "to"}: SMTP envelope incl. Bcc (v2+)
    created    INTEGER NOT NULL,
    attempts   INTEGER NOT NULL DEFAULT 0,
    last_error TEXT
);

-- Local changes not yet pushed to the server (offline flag/move/delete).
CREATE TABLE IF NOT EXISTS pending_ops (
    id      INTEGER PRIMARY KEY,
    account TEXT    NOT NULL,
    op      TEXT    NOT NULL,  -- JSON-encoded tern_sync::Op
    created INTEGER NOT NULL
);
