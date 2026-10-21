# Tasks

## 1. Building blocks
- [x] 1.1 `draft_from_raw` reads Bcc header; `tern_smtp::build_draft` keeps Bcc; verify round-trip test with Bcc in headers
- [x] 1.2 IMAP `append` returns the UID via UID SEARCH HEADER Message-ID; verify the live test against Dovecot
- [x] 1.3 Store v5 `drafts` table: put (upsert, marks not uploaded), get, uploaded(key, mailbox, remote_id, version), pending uploads, delete, find message by Message-ID in a mailbox; verify tests incl. v4 → v5 migration

## 2. Engine
- [x] 2.1 SaveDraft (with `replaces`), upload + replace old copy via Op::Destroy, one upload per key, newer-version re-upload, DiscardDraft, Send.draft_key, flush retries; demo `append`; verify engine tests with the fake backend

## 3. UI
- [x] 3.1 Compose: draft key, dirty tracking, 4 s debounce + 30 s tick, Ctrl+S, close prompt (Save / Don't Save / Cancel), send with draft key; verify in the test display (demo)
- [x] 3.2 Edit Draft: viewer banner for Drafts-mailbox messages and list activation; verify in the test display

## 4. Integration
- [x] 4.1 tests, clippy; PLAN.md, UI.md; archive

## Notes

- Verified in the test display (demo): autosave after a pause, a second save replacing the
  server copy (one copy), Ctrl+S, Save on close, Edit Draft banner, Return in the list,
  sending a reopened draft (Drafts emptied, message in Sent), Don't Save removing an
  autosaved draft. IMAP `append` returning the UID verified against the local Dovecot.
- Not exercised live: offline drafts uploading later and changing the From account
  mid-draft (engine tests cover both); real Gmail/iCloud Drafts folders.
- Found while testing: reopening a draft saved by an earlier window left that window's
  entry pointing at a server copy about to be replaced; AdoptDraft now forgets it.
