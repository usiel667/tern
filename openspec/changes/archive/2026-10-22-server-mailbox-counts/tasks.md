# Tasks

## 1. Backends
- [x] 1.1 `MailboxCounts` + `MailboxDelta.counts`; IMAP STATUS before SELECT; JMAP Mailbox/get in the sync batch; verify live test (Dovecot, Gmail)

## 2. Store
- [x] 2.1 Schema v6 count columns + migration; `apply_delta` records counts with the cache snapshot; older batches move the snapshot; `mailboxes()` derives shown counts; verify tests (read, delete, older, no counts)

## 3. UI + integration
- [x] 3.1 Unread badge from server counts; tests, clippy; check on the real Gmail cache; PLAN/DEBUG; archive
