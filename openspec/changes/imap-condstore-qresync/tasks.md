# Tasks

## 1. Cursor (tern-imap)

- [x] 1.1 Add the change counter to `ImapState` with a `v2` encoding that still decodes `v1` (modseq 0); keep the counter in `fetch_older`; verify with unit tests for v2 round-trip, v1 decode, and garbage

## 2. Extensions per connection (tern-imap)

- [x] 2.1 Wrap the cached session as `Conn { session, ext }`, read CONDSTORE/QRESYNC from CAPABILITY after login, send `ENABLE QRESYNC` when offered, and add the `#[doc(hidden)] limit_extensions` test switch; verify the existing live `full_cycle` test still passes against `dev/test-imap-server.sh`

## 3. Sync paths (tern-imap)

- [x] 3.1 Add a pure helper that turns `VANISHED` UID ranges into removed UIDs inside the cached window; verify with unit tests (single UIDs, ranges, ranges overlapping the window edges, empty)
- [x] 3.2 Implement the QRESYNC path (drain unsolicited, `CHANGEDSINCE … VANISHED`, collect `VANISHED`, skip entirely when the counter and UIDNEXT are unchanged), recording the SELECT's HIGHESTMODSEQ; verify with live tests: an idle sync returns no flags/removed/present, one flag change returns exactly that message, and a deletion on a second connection is reported in `removed`
- [x] 3.3 Implement the CONDSTORE-only path (`CHANGEDSINCE` flags + `UID SEARCH` presence) and keep the neither path for unknown counters and servers without extensions; verify by running the live scenarios with `limit_extensions` set to CONDSTORE-only and to neither
- [x] 3.4 Add the full-check safety net every 50th sync per mailbox; verify with a unit test of the counter logic
- [x] 3.5 Verify upgrade and long-session behavior with live tests: a sync from a `v1` cursor is not a full resync and returns a `v2` cursor; after 240 commands across two mailboxes on one connection, a deletion made elsewhere is still detected

## 4. Integration and docs

- [x] 4.1 Run `cargo test --workspace`, the live suite against the local Dovecot, `cargo clippy --workspace --all-targets` (warning-free) and `cargo machete`; launch `--demo` to confirm nothing changed for the demo; verify all pass
- [x] 4.2 Update `md/PLAN.md` (CONDSTORE/QRESYNC row, milestone 2 checklist, remove the "larger IMAP FLAGS window" known gap) and add the unsolicited-channel finding to `md/DEBUG.md`; verify the files describe the shipped behavior

## Workflow follow-up

- Archive the change with `/opsx:archive` after review, creating `openspec/specs/mailbox-sync/spec.md`.
