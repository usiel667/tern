# Proposal

## Why

Closing a New Message window can only discard it, and nothing is saved while writing: a
crash, a closed window or a reboot loses the message. Apple Mail saves drafts to the
account's Drafts mailbox as you type, so they're also on the phone and in webmail. This is
milestone 3's "Save to Drafts / autosave".

## What Changes

- **Autosave**: while a New Message has content, it's saved a few seconds after typing
  stops, and at least every 30 seconds while typing continues. **Ctrl+S** saves now.
- **Local first, then the server**: each save is kept on this computer immediately (a new
  `drafts` table, schema v5), then uploaded to the account's Drafts mailbox (flagged
  `\Draft` and seen). The previous server copy is deleted once the new one is up, so the
  Drafts mailbox holds one copy per message. Offline, the local copy is kept and uploaded
  when the account is back.
- **Bcc is kept** in drafts (unlike sent mail), so reopening a draft restores it.
- **Closing** a window with content asks "Save this message as a draft?" — **Save**,
  **Don't Save** (removes any draft already saved), **Cancel**. Closing an empty window
  closes it.
- **Sending** removes the draft.
- **Editing drafts**: a message in a Drafts mailbox shows an "Edit Draft" banner in the
  viewer, and double-clicking it (or Return) opens it in a New Message window, including
  drafts written in other mail apps. Saving it replaces that server copy.
- The demo account stores appended messages so drafts can be tried there.

Out of scope: rich text drafts, attachments larger than the server's limit in drafts (the
size warning applies), a separate local "unsaved drafts" list for accounts without a Drafts
mailbox (they keep the local copy only, with a notice).

## Capabilities

### New Capabilities

(none)

### Modified Capabilities
- `compose`: drafts, autosave, close prompt, editing drafts.

## Impact

- `tern-core`: `mime::draft_from_raw` also reads the Bcc header.
- `tern-smtp`: `build_draft` (keeps Bcc, fresh Message-ID per version).
- `tern-imap`: `append` reports the new message's UID (UID SEARCH HEADER Message-ID).
- `tern-store`: schema v5 `drafts` table and accessors.
- `tern-sync`: `Command::SaveDraft`/`DiscardDraft`, `Send.draft_key`, upload/replace logic,
  retry with the periodic flush; demo `append`.
- `tern` (UI): autosave timers, Ctrl+S, close prompt, Edit Draft banner and activation.
