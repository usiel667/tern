# Design

## Context

See proposal.md for motivation and specs/mailbox-sync/spec.md for required behavior.

Current IMAP sync (`ImapBackend::sync_mailbox`), with cursor `v1:<uidvalidity>:<low>:<high>`:
1. `SELECT`; if UIDVALIDITY differs → full resync of the newest `limit` messages.
2. New mail: `UID FETCH <high+1>:*` with envelope items.
3. Flags and deletions: `UID FETCH <low>:<high> (UID FLAGS)` for every cached UID, returned
   as `flags_changed` plus `present`; `Store::apply_delta` deletes cached rows not in `present`.

Library facts (async-imap 0.12 / imap-proto 0.17):
- `select_condstore` sends `SELECT … (CONDSTORE)` and parses `HIGHESTMODSEQ` into
  `Mailbox::highest_modseq`; `Fetch::modseq` is parsed.
- `UID FETCH` accepts arbitrary modifiers in its query string, so
  `(UID FLAGS) (CHANGEDSINCE m VANISHED)` works; the FETCH items come back on the stream.
- `VANISHED` responses are not FETCH items: they go to `Session::unsolicited_responses`, a
  channel bounded at 100, as `UnsolicitedResponse::Other` (imap-proto parses them as
  `Response::Vanished { earlier, uids }`). Tern never reads that channel today, so on a
  long-lived connection it fills with other notices and later sends are dropped silently.
- `Session::capabilities()` and `run_command_and_check_ok("ENABLE QRESYNC")` are available.

## Goals / Non-Goals

**Goals:**
- Sync cost proportional to changes on CONDSTORE servers; near zero for idle QRESYNC mailboxes.
- Same `MailboxDelta` contract to the engine (no engine/store changes).
- Old cursors keep working; no forced re-download on upgrade.

**Non-Goals:**
- `SELECT … (QRESYNC (…))` (folding the resync into SELECT). It returns the same data via
  unsolicited responses that would need custom parsing; an explicit `UID FETCH` is simpler.
- `UNCHANGEDSINCE` on STORE (conflict detection for pushes).

## Decisions

### 1. Cursor v2 adds the change counter
`v2:<uidvalidity>:<low>:<high>:<modseq>`. `decode` accepts v1 (modseq = 0, meaning
"unknown"). A cursor with modseq 0 does one full flag check (today's path) and then records the
counter, so upgraded caches switch over on their second sync without re-downloading.
`fetch_older` keeps the cursor's modseq when it lowers `low` (see §5 for why that is safe).

### 2. Detect extensions once per connection
The cached command connection becomes `Conn { session, ext: Extensions }`, where
`Extensions { condstore, qresync }` is read from `CAPABILITY` after login. If QRESYNC is
offered, `ENABLE QRESYNC` is sent once (it implies CONDSTORE). A mailbox whose SELECT reports
no `HIGHESTMODSEQ` (NOMODSEQ) is treated as having neither for that sync. A `#[doc(hidden)]`
`ImapBackend::limit_extensions` lets tests force the CONDSTORE-only and neither paths against
a server that offers both.

### 3. The record point is the SELECT's HIGHESTMODSEQ
The new cursor stores the `HIGHESTMODSEQ` from the SELECT that started the sync, never a value
seen later. Changes made during the sync have a higher modseq and are reported again next time
(idempotent), so nothing is skipped.

### 4. Three paths after new-mail fetch
- **QRESYNC, counter unchanged and `UIDNEXT <= high + 1`**: nothing to fetch; delta has no
  flags, no `present`, no `removed`; skip the new-mail fetch too.
- **QRESYNC**: drain the unsolicited channel, then
  `UID FETCH <low>:<high> (UID FLAGS) (CHANGEDSINCE <m> VANISHED)`; FETCH items become
  `flags_changed`; afterwards drain the channel again and collect `VANISHED` UIDs inside
  `<low>:<high>` into `removed`. `present` stays `None`.
- **CONDSTORE only**: `UID FETCH <low>:<high> (UID FLAGS) (CHANGEDSINCE <m>)` for flags;
  `UID SEARCH UID <low>:<high>` (numbers only) for `present`, because CONDSTORE alone doesn't
  report expunges.
- **Neither / modseq unknown**: today's full `UID FETCH <low>:<high> (UID FLAGS)` + `present`.
The range expansion and filtering of `VANISHED` UIDs is a pure function with unit tests.

### 5. Older mail stays covered
Older messages loaded by `fetch_older` arrive with their current flags. Any later change has a
modseq above the cursor's (which was recorded before or at that fetch), and the next sync's
`CHANGEDSINCE` / `VANISHED` range is `<low>:<high>` with the lowered `low`, so their flag
changes and deletions are picked up (mail-history "Older mail is kept in sync").

### 6. Drain before each sync command
Draining immediately before the `CHANGEDSINCE … VANISHED` fetch empties the 100-slot channel,
so the `VANISHED (EARLIER)` reply has room. Other unsolicited notices are discarded; they
describe changes this sync re-reads anyway.

Measured during implementation: against Dovecot, with Tern re-selecting the mailbox for each
command, the channel holds at most one item (the `ENABLED` reply or a `VANISHED`); `[CLOSED]`
and other per-SELECT responses don't reach it, and the live long-session test passes with or
without the pre-drain. The drain stays as cheap insurance for servers that send more
unsolicited notices, not as a fix for an observed failure.

## Risks / Trade-offs

- [A server advertises QRESYNC but omits `VANISHED (EARLIER)`] → deletions would be missed.
  Mitigation: a full check (neither path) every 50th sync of a mailbox (`sync_count` kept in
  memory per backend); cheap insurance, removed if never needed.
- [More round trips for CONDSTORE-only servers than today when many flags change] → at most
  one extra `UID SEARCH`; still far less data than fetching every message's flags.
- [`ENABLE QRESYNC` changes EXPUNGE notices to VANISHED on that connection] → the IDLE watcher
  uses its own connection without ENABLE, so push is unaffected.

## Migration Plan

Cursors upgrade in place (§1); no schema change. Rolling back: an older build can't decode v2
cursors and does a one-time full resync of each mailbox — acceptable for a rollback.
