# Design

## Context

See proposal.md for motivation and specs/mail-history/spec.md for required behavior.

Current state that shapes the approach:

- `Engine::sync_mailbox` asks the backend for a `MailboxDelta` with `limit =
  INITIAL_SYNC_LIMIT` (1,000) and stores it with `Store::apply_delta`, which also writes the
  mailbox's `sync_state`. A `syncing: HashSet<MailboxId>` stops two syncs of one mailbox
  overlapping.
- The IMAP cursor is `v1:<uidvalidity>:<low_uid>:<high_uid>`. Incremental syncs fetch new
  UIDs above `high_uid` and FLAGS for `low_uid..high_uid`, and return `present` for that
  window; `apply_delta` deletes cached rows of the mailbox that aren't in `present`.
- The JMAP cursor is the account-wide `Email/changes` state; `Email/changes` already reports
  changes to any message, cached or not.
- The UI (`message_list::reload`) loads up to 2,000 rows from the store for the current
  `ListScope`; there is no scroll handling.
- `apply_delta` followed by `overlay_pending` (from `mail-actions`) is the one way server data
  enters the store; older batches should use the same path.

## Goals / Non-Goals

**Goals:**
- One backend method for "the next batch older than what I have", reusing `MailboxDelta`.
- No race between loading older mail and the regular sync of the same mailbox.
- Cheap UI paging over the cache before any network request.

**Non-Goals:**
- Reducing the per-sync cost of a larger IMAP window (FLAGS for `low..high` grows with history;
  CONDSTORE/QRESYNC in the next milestone-2 step addresses it).
- Persisting "mailbox fully downloaded" across restarts.

## Decisions

### 1. Backend API: `fetch_older` returns a delta plus "more"
```text
async fn fetch_older(&self, mailbox, since: &SyncState, from: &OlderFrom, count) -> Result<OlderBatch>
OlderFrom  { oldest_remote_id: Option<String>, cached: u32 }
OlderBatch { delta: MailboxDelta /* added + new_state only */, more: bool }
```
The default implementation returns an empty batch with `more: false` and the cursor unchanged,
so test fakes and future backends compile. Returning a `MailboxDelta` lets the engine store it
with `apply_delta` (upserts, cursor write, rethread, FTS triggers) and then `overlay_pending`.
`present` stays `None` so nothing is deleted. Alternative: a separate store insert path —
rejected as duplicate logic.

### 2. IMAP: UID SEARCH below the window
`UID SEARCH UID 1:<low_uid - 1>` returns the older UIDs (numbers only, cheap even for large
mailboxes); take the highest `count`, `UID FETCH` their envelopes, and lower `low_uid` to the
smallest fetched. `more` = the search returned more than `count`. If `low_uid <= 1` there is
nothing older. If the cursor's UIDVALIDITY doesn't match the server's, return an empty batch
with the cursor unchanged and `more: true`; the next regular sync performs the full resync.
Because `low_uid` moves down, later incremental syncs check FLAGS and expunges for the older
messages too (spec: "Older mail is kept in sync"). The UID selection is a pure function so it
can be unit tested without a server.

### 3. JMAP: `Email/query` anchored at the oldest cached message
Same filter and sort as the initial sync (`inMailbox`, `receivedAt` descending) with
`anchor = oldest_remote_id`, `anchorOffset = 1`, `limit = count`, `calculateTotal = true`.
If the anchor is gone (`anchorNotFound`), fall back to `position = cached`. `more` =
`position + returned < total`. The cursor is returned unchanged: `Email/changes` already
covers every message. Alternative: position only — rejected because new mail or deletions
shift positions and could skip messages.

### 4. Engine: one slot per mailbox for syncs and older loads
`Command::LoadOlder(Vec<MailboxId>)`. For each mailbox: skip if it's in `exhausted` (session
only) or not selectable; if it's in `syncing`, record it in `older_wanted` and start the load
when that sync's `Delta` arrives; otherwise insert into `syncing` and spawn the fetch. The
result comes back as `Internal::Older { account, mailbox, result }` (not `Failed`, whose
mailbox-scoped rollback would re-download the mailbox). On success: `apply_delta`,
`overlay_pending`, `exhausted` if `!more`, emit `MessagesChanged` / `MailboxesChanged`. On
error: release the slot; `Network` silently, `Auth` via `schedule_reconnect`, others toast.
A `LoadOlder` for a mailbox already loading older mail is ignored (spec: no duplicate
download). Sharing the `syncing` slot is what prevents a regular sync's older cursor from
overwriting the lowered `low_uid` (which would make the next sync delete the older rows via
`present`). `OlderFrom` is read from the store (oldest cached remote id, count) when the
load starts.

### 5. UI: grow the cache window first, then ask the engine
`MessageList` gains a list limit, starting at 500 and reset when the scope or search changes.
On the scroller's `edge-reached` at the bottom: if the store returned as many rows as the limit,
raise the limit by 500 and reload (no network). Otherwise every cached row is shown: for
`ListScope::Mailbox(m)` send `LoadOlder([m])`; for `ListScope::Role(r)` send the ids of all
selectable mailboxes with role `r`; other scopes and active searches send nothing. The
`MessagesChanged` event already triggers a reload, so new rows appear without extra wiring.

### 6. Demo: a window over generated history
The demo inbox gains ~600 generated older messages. The demo backend emulates a server window:
the first sync returns only the newest 50 per mailbox (a demo-only constant, so the feature is
visible without 1,000+ messages), the cursor stores the window's start, `present` covers the
window, and `fetch_older` extends it.

## Risks / Trade-offs

- [Larger IMAP windows make every incremental sync fetch more FLAGS] → acceptable until
  CONDSTORE/QRESYNC; the user only grows the window by scrolling.
- [JMAP position fallback can skip a message if mail is deleted between batches] → only used
  when the anchor itself was deleted; a later full resync corrects it.
- [`edge-reached` doesn't fire if the list is shorter than the view] → only possible when a
  mailbox has fewer cached messages than fit on screen, which means the initial sync already
  got everything (it fetches up to 1,000).
- [UID SEARCH on very large mailboxes returns many numbers] → numbers only, one round trip,
  typically well under a megabyte even for 100k messages.

## Migration Plan

No schema change; IMAP and demo cursors keep their formats (the demo's is rebuilt on first
sync). Rolling back leaves extra older rows in the cache, which the older build keeps syncing
within the lowered window.
