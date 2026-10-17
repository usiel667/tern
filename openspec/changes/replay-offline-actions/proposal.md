# Proposal

## Why

Flagging, marking read, moving, archiving or deleting mail while offline (or when the
connection drops mid-command) is silently undone: the push fails, the engine resyncs the
mailbox, and the server's old state overwrites the change. Apple Mail queues these actions
and applies them when it reconnects; Tern should too. The `pending_ops` table was created in
milestone 1 for this but is never written or read.

## What Changes

- Every flag/move/delete action is recorded in a durable per-account queue before it is
  pushed, and removed only once the server accepts it. The queue survives restarts.
- Actions are pushed to the server in the order they were made, one account at a time.
- When the server can't be reached (offline, connection dropped, sign-in failing), the
  action stays queued and the local change stays visible. The queue is retried when the
  account next talks to the server (reconnect, the 5-minute sync, or app start).
- When the server rejects an action outright (e.g. the folder no longer exists), that
  action is dropped and the mailbox is resynced so the local view matches the server, as
  today, with an error shown.
- While actions are waiting, syncing a mailbox no longer reverts them on screen: queued
  flag changes are re-applied over server data, and messages with a queued move or delete
  stay hidden from their old mailbox.
- If a mailbox's message ids are invalidated (IMAP UIDVALIDITY change), queued actions for
  that mailbox are dropped rather than applied to the wrong messages.
- The IMAP backend reports dropped connections and I/O failures as network errors instead
  of protocol errors, so the engine can tell "offline" from "server said no".
- Removing an account discards its queued actions.

Out of scope: an "offline / N changes waiting" indicator in the UI, Undo (Ctrl+Z), and
queueing for sends (the outbox already does that).

## Capabilities

### New Capabilities
- `mail-actions`: how user actions on messages (flags, read state, move, archive, junk,
  delete) are applied locally and delivered to the server, including offline queueing,
  ordering, rejection handling and their effect on sync.

### Modified Capabilities

(none; no specs exist yet)

## Impact

- `tern-sync`: `set_flags` / `move_messages` go through a new per-account queue and
  drain; new `Op` type (serialized into `pending_ops.op`); overlay of queued ops after
  `apply_delta`; error classification for retry vs drop.
- `tern-store`: queue functions over the existing `pending_ops` table, plus helpers to set
  flags and delete by `(mailbox, remote_id)`. No schema version bump is expected: the table
  already exists with the needed columns.
- `tern-imap`: map `async-imap` I/O and connection-lost errors to `Error::Network`.
- `tern-sync` regains `serde` / `serde_json` as dependencies (removed as unused earlier).
- UI (`tern` crate): no changes required; fewer error toasts while offline.
