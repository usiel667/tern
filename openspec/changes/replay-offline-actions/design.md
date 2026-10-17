# Design

## Context

See proposal.md for motivation and specs/mail-actions/spec.md for the required behavior.

Current state that shapes the approach:

- `Engine::set_flags` / `Engine::move_messages` apply the change to the store, then spawn
  one task per `(account, mailbox)` group that pushes it. Any error becomes
  `Internal::Failed { mailbox: Some(..) }`, which calls `store_resync` (drop the cursor and
  re-download) and toasts. That resync is what undoes offline edits.
- The IMAP backend maps every `async_imap::Error` from a command to `Error::Protocol`
  (`map_err(proto)`); only connection setup produces `Error::Network`. A dropped connection
  mid-command is indistinguishable from a server `NO`.
- `Store::apply_delta` overwrites flags for every message in the IMAP UID window on each
  incremental sync, and a full resync re-inserts every message, so pushing later can't
  stop the screen reverting in between.
- `full_resync` is set both when IMAP UIDVALIDITY changes and when the engine drops the
  cursor itself (`store_resync`), so it can't be used alone to detect invalidated ids.
- `pending_ops (id, account, op TEXT, created)` exists in the schema but is unused.

## Goals / Non-Goals

**Goals:**
- One durable, ordered, per-account queue that every message action goes through, online
  or offline (no separate "offline mode").
- Clear retry-vs-drop rule based on error kind.
- Local view stays consistent with the queue across syncs and restarts.

**Non-Goals:**
- Merging or collapsing queued ops (e.g. read then unread → nothing). Replaying both is
  correct and cheap.
- A retry cap or "stuck change" UI for ops that fail with network errors forever.
- New UIDs for moved messages (COPYUID parsing); destination mailboxes resync as today.

## Decisions

### 1. Every action goes through the queue (queue-first), not only failed ones
`set_flags` / `move_messages` keep their optimistic local update, then append an `Op` to
`pending_ops` and call `drain(account)`. Alternative: push directly and enqueue only on
failure. Rejected because a later action could reach the server before an earlier one that
is waiting in the queue, breaking ordering, and because a crash between the local update
and the push would lose the action. This mirrors the outbox, which is already queue-first.

### 2. One drain per account, one op at a time, in insertion order
The engine keeps `draining: HashSet<AccountId>`. `drain` takes the oldest op for the
account, runs it in `spawn_task` (so the busy spinner still works) and returns
`Internal::OpDone { account, id, op, result }`. On completion the loop continues with the
next op. Serializing per account gives the ordering guarantee simply; accounts still drain
in parallel. Cost: a burst of actions takes one round trip each, which is acceptable for
user-paced actions. Bulk actions on a selection are already one op (grouped by mailbox and
by resulting flag set, as today).

### 3. `Op` stores remote identifiers plus the local mailbox id
```text
Op::SetFlags { mailbox: MailboxId, mailbox_remote: String, items: Vec<(remote_id, Flags)> }
Op::Move     { from: MailboxId, from_remote: String, to_remote: String, remote_ids: Vec<String> }
Op::Destroy  { mailbox: MailboxId, mailbox_remote: String, remote_ids: Vec<String> }  // delete in Trash
```
Serialized as JSON into `pending_ops.op` (serde derive; `tern-sync` regains `serde` and
`serde_json`). Local `MessageId`s are not stored because message rows are deleted and
re-inserted by syncs; `(mailbox, remote_id)` is the stable key. `Flags` are stored as the
full resulting set, which makes replay idempotent (and is how `set_flags` already pushes).

### 4. Retry vs drop by error kind
- `Network`, `Auth` → keep the op, stop draining this account, no toast. `Auth` also goes
  through the existing reconnect backoff. The next successful `Internal::Mailboxes` for the
  account (app start, reconnect, 5-minute sync) calls `drain` again.
- Anything else (`Protocol`, `NotFound`, `Unsupported`, `Config`) → delete the op, toast,
  `store_resync` the op's mailbox (the existing rollback), and continue draining.
- `Storage` errors come from our own database, not the server; treat as keep + toast.

### 5. Fix IMAP error classification at the source
Replace the blanket `map_err(proto)` with a classifier: `async_imap::Error::Io` and
`ConnectionLost` → `Error::Network`; `No`, `Bad` and the rest → `Error::Protocol`. Done in
`tern-imap` so every caller benefits; `with_session` already drops the connection for both.
JMAP already maps transport failures to `Network` and method errors to `Protocol`.

### 6. Re-apply the queue after every delta (overlay)
After `apply_delta` for mailbox M, the engine walks the account's queued ops in order and
re-applies those touching M: `SetFlags` → set flags by `(M, remote_id)`; `Move`/`Destroy`
from M → delete local rows by `(M, remote_id)`. New store helpers do this by remote id.
Alternative: make `apply_delta` skip rows with pending ops. Rejected: it would put
knowledge of the op format into `tern-store`; the overlay keeps the store generic.

### 7. Detect invalidated ids explicitly
Add `ids_invalidated: bool` to `MailboxDelta`. IMAP sets it only when a previous cursor
decoded successfully and its UIDVALIDITY differs from the server's. The engine then deletes
queued ops for that mailbox (with one toast saying how many) before the overlay. Cursor drops
done by `store_resync` decode as "no previous cursor", so they don't trigger it. JMAP never
sets it (ids are stable).

### 8. Moves keep refreshing the destination
On a successful `Move`, `OpDone` triggers `sync_mailbox` for the destination (today this is
done through a synthetic `PushEvent`), so the moved message appears there with its new id.

## Risks / Trade-offs

- [UIDVALIDITY changes during the same sync that follows a `store_resync` cursor drop] →
  the change isn't detected and queued ops for that mailbox could target renumbered UIDs.
  Requires a server renumbering and a rejected action on the same mailbox at once; IMAP
  servers renumber very rarely. Accepted.
- [An op that always fails with a network-class error, e.g. a server bug that drops the
  connection on one command] → blocks that account's queue behind it. Mitigation: logged
  with `tracing::warn`; a retry cap is listed as a non-goal for a follow-up.
- [Serialized draining is slower for rapid successive actions] → one op per round trip;
  bulk selections are still one op. Accepted.
- [IMAP UID commands on UIDs that no longer exist succeed silently] → an action on a message
  deleted elsewhere is a no-op rather than a rejection. That matches Apple Mail and needs no
  handling.

## Migration Plan

No schema version bump: `pending_ops` exists in v1 and v2 databases and is empty. Rolling
back to an older build leaves any queued rows unread and harmless. `MailboxDelta` gains a
defaulted field, so other backends and test fakes compile unchanged.
