# Proposal

## Why

Every IMAP sync (each push, every 5 minutes, and on opening a mailbox) re-downloads the
flags of every cached message in the mailbox, whether anything changed or not: 1,000
messages per inbox at minimum, more after scrolling back through history. Servers that
support CONDSTORE (RFC 7162) can report only what changed since a change counter, and
QRESYNC adds the list of deleted messages, so an idle mailbox costs almost nothing to check.
Most major servers support CONDSTORE (Dovecot, Fastmail, iCloud, Gmail), and Dovecot and
Fastmail also support QRESYNC.

## What Changes

- The IMAP sync cursor records the mailbox's change counter (`HIGHESTMODSEQ`); existing
  cursors upgrade on their next sync.
- On servers with QRESYNC: a sync asks only for messages changed since the stored counter
  and gets deletions as a list, and skips the flag and deletion check entirely when the
  counter hasn't moved.
- On servers with CONDSTORE but not QRESYNC: changed flags only, plus a cheap list of the
  message ids still present (numbers only) to detect deletions.
- Servers with neither keep today's behavior.
- Deletion notices from the server are read reliably: the connection's notice buffer is
  cleared before each sync, so it can't overflow over a long session and drop them.
- Mail that arrives during a sync, and older mail loaded by scrolling, stay covered: the
  stored counter only moves forward to what the server reported for the window that was
  actually checked.

Out of scope: JMAP (it already syncs by change state), using these extensions when pushing
flag changes, and Gmail's labels extension.

## Capabilities

### New Capabilities
- `mailbox-sync`: how the cached copy of a mailbox is kept up to date with the server
  (new mail, flag changes, deletions, renumbering) and what a sync costs.

### Modified Capabilities

(none; `mail-history`'s "Older mail is kept in sync" stays as written and is preserved)

## Impact

- `tern-imap`: cursor format v2 (adds the change counter; v1 still decodes), capability
  detection after login (and `ENABLE QRESYNC` when offered), `CHANGEDSINCE` / `VANISHED`
  fetches, `UID SEARCH` for presence on CONDSTORE-only servers, draining unsolicited
  responses. `sync_mailbox` keeps its signature; `MailboxDelta` is unchanged.
- No engine, store or UI changes expected.
- Testing: the local Dovecot test server (`dev/test-imap-server.sh`) supports both
  extensions; a test-only switch exercises the CONDSTORE-only and neither paths against it.
