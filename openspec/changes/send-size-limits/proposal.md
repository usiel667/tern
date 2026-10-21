# Proposal

## Why

Tern only warns at a fixed 25 MB of attachments. Servers differ (Gmail advertises
35,882,577 bytes for the whole encoded message, iCloud 28,319,744 — about 20 MB of
attachments), attachments grow by about
a third when encoded, and a message the server refuses is treated like a network hiccup and
retried every 5 minutes forever, invisibly. Worse, on JMAP a refused submission is never
noticed: `EmailSubmission/set` errors are ignored, so the message is dropped from the
outbox as if sent. The owner's accounts are Gmail and iCloud, so limits must be right for
them, and general for any server.

## What Changes

- **Learn each account's real limit.** IMAP accounts: the SMTP server's `SIZE` from EHLO
  (probed without signing in, once per connection of the account). JMAP accounts:
  `maxSizeUpload` from the session. The engine reports it to the UI.
- **Check on Send.** The New Message window builds the real message and compares its size
  with the From account's limit; if it's over, the window stays open and explains
  ("This message is 41.2 MB, but iCloud accepts messages up to 27.0 MB"). Unknown limit: no block.
- **Attach-time warning uses the real limit** (estimated encoded size vs. the From account's
  limit), falling back to 25 MB of attachments when the limit is unknown.
- **Permanent rejections stop retrying.** A new `Error::Rejected` means "the server refused
  this message; sending it again won't help": SMTP 5xx replies other than sign-in failures,
  JMAP `notCreated` from `Email/import`/`EmailSubmission/set`, HTTP 413 on upload, and
  messages over a known limit (checked before connecting). Sign-in rejections (SMTP 530,
  534, 535, 538) stay `Auth` and keep the existing reconnect path.
- **Rejected messages are kept, visible and fixable.** They're marked in the outbox
  (schema v4) and no longer retried. A notice says "Couldn't send “Subject”: reason" with a
  Review button; an Unsent Messages dialog (also in the main menu, and offered at startup
  when any exist) lists them with **Edit** (reopens a New Message with recipients, Bcc,
  subject, text and attachments, and removes it from the outbox), **Try Again**, and
  **Delete**.

Out of scope: an Outbox mailbox in the sidebar, a retry limit for temporary (network)
failures, image resizing, share links for big files.

## Capabilities

### New Capabilities
- `sending`: how outgoing mail is size-checked, submitted, and what happens when a server
  refuses it.

### Modified Capabilities

(none)

## Impact

- `tern-core`: `Error::Rejected`; `MailBackend::send_limit` (default `None`);
  `mime::draft_from_raw` to rebuild a `Draft` from a queued message.
- `tern-smtp`: `size_limit(server)` EHLO probe, SMTP reply classification.
- `tern-jmap`: session capabilities, `send_limit`, `notCreated`/413 handling.
- `tern-store`: schema v4 (`outbox.rejected`), `outbox_reject`/`outbox_rejected`/
  `outbox_retry`/`outbox_item`.
- `tern-sync`: limit probing, `Event::SendLimit`/`SendRejected`/`OutboxChanged`,
  `Command::RetrySend`/`DiscardSend`, pre-flight size check.
- `tern` (UI): send-time check, limit-aware warning, rejection notice, Unsent Messages
  dialog; depends on `tern-smtp` to measure the real message.
