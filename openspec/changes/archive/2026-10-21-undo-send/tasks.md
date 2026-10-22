# Tasks

## 1. Engine
- [x] 1.1 `Command::Send.hold`, `Command::UndoSend`, `Event::SendUndone`/`SendReleased`; held table + timers; flush skips held; verify engine tests (undo, release sends with reply flag, restart sends a leftover)

## 2. UI
- [x] 2.1 Send hides the window and shows "Sending…" with Undo; undo reopens and re-saves the draft; release closes it; verify in the test display (demo)

## 3. Integration
- [x] 3.1 tests, clippy; PLAN.md, UI.md; archive

## Notes

- Verified in the test display (demo): Send shows "Sending “Undo test”…" with Undo and
  removes the autosaved draft; Undo reopens the window with its text and re-saves the draft;
  sending again and waiting shows "Message sent", closes the window, and Sent has exactly one
  copy. Focus on the real desktop unchanged.
- Engine tests cover undo, release with the reply flag, the flush skipping held messages,
  too-late undo, and a hold interrupted by quitting being sent on the next start.
- Not exercised live: quitting during the hold, real Gmail/iCloud.
- Found while testing: `test-display.sh keys` couldn't find windows (whole-title match);
  fixed, see DEBUG.md.
