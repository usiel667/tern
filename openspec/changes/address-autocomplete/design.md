# Design

## Context

The UI has a read-only `Store` on the main thread; the engine owns the writer and calls
`apply_delta` for every batch of synced messages. Addresses are stored per message as JSON
(`from_json`, `to_json`, `cc_json`) plus a lossy `from_text`/`to_text` for FTS.

## Decisions

- **A maintained `contacts` table, not a query over `messages`.** Scanning and JSON-decoding
  tens of thousands of rows per keystroke is too slow on the UI thread; a small keyed table
  with counts answers a prefix query in well under a millisecond.
- **Count on insert only.** `apply_delta` checks whether `(mailbox, remote_id)` already exists
  before its upsert and records contacts only for genuinely new rows, so flag-only re-syncs
  don't inflate counts. Counts can still drift upwards (a move re-inserts the message in the
  destination; a full resync re-inserts a mailbox; Gmail labels appear in several folders).
  That's acceptable: counts only rank suggestions, and nothing decrements them.
- **Role decides direction.** The store knows the mailbox role but not the user's
  identities, so "I wrote to them" = recipients of messages in Sent. Gmail's All Mail counts
  its From like any other folder, which records the user's own address; the UI filters own
  addresses out (it has the account identities).
- **Ranking:** `ORDER BY sent > 0 DESC, sent * 4 + received DESC, last_seen DESC`. People
  you've written to beat frequent senders, which keeps newsletters below colleagues.
- **Matching in SQL with `LIKE`**: `email LIKE p%` OR `name LIKE p%` OR `name LIKE '% p%'`,
  with `%`, `_` and `\` escaped. ASCII-case-insensitive, which is enough for a prefix.
- **UI: `gtk::Popover` + `ListBox`, not `EntryCompletion`** (deprecated in GTK 4.10). The
  popover has `autohide(false)` and isn't focusable so typing stays in the entry; a
  capture-phase key controller on the entry handles Up/Down/Return/Tab/Escape only while the
  popover is showing, so Ctrl+Return and the window's Escape keep working otherwise. The
  popover is unparented when the compose window is destroyed.
- **Backfill in the v2→v3 migration**, in Rust (JSON parse), in one transaction. A fresh
  database starts empty and fills as mail syncs.

## Risks / Trade-offs

- A spammer you never moved to Junk will appear in suggestions if they wrote to you, ranked
  below anyone you've written to. Forgetting an entry is a follow-up.
- `LIKE` doesn't fold non-ASCII case ("é" vs "É"); acceptable for prefixes.
