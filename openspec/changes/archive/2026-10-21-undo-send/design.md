# Design

## Where the hold lives

In the **engine**, not the UI. The message is built and written to the outbox when Send is
pressed (as today), so nothing is lost if Tern crashes or quits during the hold. The engine
keeps an in-memory table `held: key → (outbox id, account, message, reply flag)` and a timer
per entry (a plain `tokio::spawn` sleep, not counted as in-flight work so it doesn't keep
"syncing" spinners or tests waiting). The outbox flush skips held ids.

- Timer fires → entry removed → `Event::SendReleased(key)` → dispatched as a normal send.
- `UndoSend(key)` while held → entry removed, outbox row deleted → `Event::SendUndone(key)`.
  After release it does nothing (too late; the UI already knows from `SendReleased`).
- After a restart `held` is empty, so a row left from a hold is just an unsent outbox item
  and goes out with the startup flush. That is the safe direction: the user did press Send.

The key is the compose window's draft key (unique per window).

## The window during the hold

Send hides the window instead of closing it and keeps it in a table in `compose.rs`. On
`SendUndone` it is shown again with its state intact, and marked unsaved (the engine
removed its draft when the message was queued) so it is saved to Drafts again right away.
On `SendReleased` it is closed for good. Hiding keeps every detail — attachments, the
message it replies to (still marked Answered when finally sent), From account — without
rebuilding the window from the raw message.

A hidden window keeps the GTK application alive, so closing the main window during the hold
lets the message go out before Tern exits. Ctrl+Q quits at once; the message is then sent on
the next start.

## Alternatives

- Delay in the UI only (send after a GLib timeout): loses the message on crash/quit.
- `send_after` column in the outbox: survives restarts as a hold, but undo can't work after
  a restart anyway (the window is gone), so it adds a migration for nothing.
