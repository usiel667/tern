# Proposal

## Why

Unread and total counts come from the local cache, which holds only the newest 1000 messages
of each mailbox at first. On the user's Gmail the Inbox showed 875 unread where Gmail shows
4,832. Apple Mail shows the server's numbers.

## What Changes

- Each mailbox sync also asks the server for the mailbox's message and unread counts: IMAP
  `STATUS (MESSAGES UNSEEN)` on the same connection just before `SELECT`, JMAP `Mailbox/get`
  (`totalEmails`, `unreadEmails`) in the same request.
- The store keeps those counts with a snapshot of the cache's own counts at that moment
  (schema v6). Shown count = server count + (cache count now − cache count at snapshot), so
  reading or deleting in Tern changes the badge at once, and the next sync corrects any drift.
  Loading older mail moves the snapshot (the server already counted those messages).
- Mailboxes the server hasn't reported counts for (demo, failures) keep cache counts.
- The Unread smart mailbox badge uses server counts too: per account, All Mail's unread when
  there is one (Gmail: each message counted once), otherwise the sum over mailboxes except
  Trash and Junk.

Out of scope: a server count for Flagged (no IMAP equivalent without searching every mailbox).

## Capabilities

### Modified Capabilities
- `mailbox-sync`: mailbox counts come from the server.

## Impact

- `tern-core`: `MailboxCounts`, `MailboxDelta.counts`.
- `tern-imap`, `tern-jmap`: report counts while syncing.
- `tern-store`: schema v6 columns on `mailboxes`; `apply_delta` records, older batches adjust.
- `tern` (UI): Unread badge.
