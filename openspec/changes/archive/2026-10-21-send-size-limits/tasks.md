# Tasks

## 1. Core, SMTP, JMAP
- [x] 1.1 `Error::Rejected`; `MailBackend::send_limit` default `Ok(None)`; `mime::draft_from_raw(raw, envelope_rcpts)`; verify unit test round-trip build → draft_from_raw (To/Cc/Bcc, subject, text, attachment, in-reply-to)
- [x] 1.2 `tern_smtp::size_limit` (EHLO probe, `parse_size`), reply classification (auth codes → Auth, other 5xx → Rejected, rest → Network); verify unit tests for parse and classification, and an ignored live test probing smtp.gmail.com and smtp.mail.me.com without credentials
- [x] 1.3 JMAP: session capabilities → `send_limit`; `notCreated` in import/submission → Rejected; 413 → Rejected; verify unit tests on canned responses

## 2. Store
- [x] 2.1 Schema v4 `outbox.rejected` + migration; `outbox()` excludes rejected; `outbox_reject`, `outbox_rejected`, `outbox_retry`, `outbox_item`; verify tests incl. v3 → v4 migration

## 3. Engine
- [x] 3.1 Probe limits on AccountReady → `Event::SendLimit`; pre-flight size check; Rejected → mark + `Event::SendRejected`; `Command::RetrySend`/`DiscardSend`; `Event::OutboxChanged`; verify engine tests with the fake backend: rejected not retried by flush, network failure still retried, auth failure reconnects, retry/discard work, over-limit fails before submit

## 4. UI
- [x] 4.1 Limits map; Send-time exact check with dialog; limit-aware attach warning; verify in the test display (demo with a small fake limit)
- [x] 4.2 Rejection toast with Review; Unsent Messages dialog (Edit/Try Again/Delete), main-menu entry, startup notice; verify in the test display with a demo backend that rejects a marked message

## 5. Integration
- [x] 5.1 tests, clippy, audit; PLAN.md (feature table, known gaps), md/UI.md; archive

## Notes

- Verified in the test display (demo server: 10 MB limit, refuses "[reject]" subjects):
  limit-aware attach warning, "Message Too Large" (26 MB file → 35.6 MB message), refusal
  toast, Unsent Messages via the main menu, Edit restoring To/Bcc/subject/text with one
  signature. Not exercised live: the startup notice (demo databases are fresh each run) and
  the Try Again / Delete buttons (engine tests cover both commands).
- Found and fixed on the way: Ctrl+Return in the body inserted a newline instead of sending.
