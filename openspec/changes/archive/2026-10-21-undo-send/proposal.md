# Proposal

## Why

A message sent by mistake (wrong recipient, missing attachment, sent half-written) can't be
taken back once it leaves. Apple Mail holds a sent message for a few seconds and offers
**Undo Send**, which reopens the message as it was. This is milestone 3's "Undo Send".

## What Changes

- **Hold**: Send closes the window, but the message waits **10 seconds** before it goes out
  (Apple Mail's default). A notice "Sending “Subject”…" with an **Undo** button shows for
  that time.
- **Undo** reopens the same window exactly as it was (recipients, attachments, the reply it
  answers), and saves it to Drafts again.
- **Durable**: the held message is already in the outbox, so a crash or quitting during the
  hold doesn't lose it: it is sent the next time Tern starts (it can no longer be undone).
- After the hold, sending works as before (size checks, refused messages, retries).
- The delay is a constant for now; Settings (later) will offer Off / 10 / 20 / 30 seconds.

Out of scope: Edit ▸ Undo Send (Ctrl+Z) in the main window, a setting for the delay, Send
Later (scheduled send).

## Capabilities

### New Capabilities

(none)

### Modified Capabilities
- `sending`: sent messages are held briefly and can be taken back.

## Impact

- `tern-sync`: `Command::Send.hold`, `Command::UndoSend(key)`, `Event::SendUndone(key)` /
  `Event::SendReleased(key)`; held messages skipped by the outbox flush until released.
- `tern` (UI): Send hides the compose window during the hold; Undo toast; reopen on undo,
  close on release.
