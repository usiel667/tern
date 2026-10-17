# Tasks

## 1. Error classification (tern-imap)

- [x] 1.1 Add a classifier that maps `async_imap::Error::Io` and `ConnectionLost` to `Error::Network` and everything else to `Error::Protocol`; use it in place of `map_err(proto)` for command errors; verify with a unit test on each variant
- [x] 1.2 Set `MailboxDelta::ids_invalidated` (new defaulted field in `tern-core`) only when a decoded previous cursor's UIDVALIDITY differs from the server's; verify `cargo build --workspace` passes and other backends/fakes need no change

## 2. Queue storage (tern-store)

- [x] 2.1 Add queue functions over `pending_ops`: push (returns id), list for an account in insertion order, remove by id, clear an account; verify with a store unit test covering order and clear
- [x] 2.2 Add helpers to set flags and delete rows by `(mailbox, remote_id)`; verify with a store unit test

## 3. Ops and draining (tern-sync)

- [x] 3.1 Define the serializable `Op` enum (`SetFlags`, `Move`, `Destroy`) per design §3 and re-add `serde`/`serde_json` to `tern-sync`; verify a JSON round-trip test and that `cargo machete` reports no unused dependencies
- [x] 3.2 Make `set_flags` and `move_messages` apply locally, enqueue an `Op`, and call `drain`; remove their direct push tasks; verify the existing `mark_read_is_optimistic_and_pushed` and `delete_moves_to_trash_then_trash_delete_expunges` tests still pass
- [x] 3.3 Implement `drain` (one op at a time per account, `Internal::OpDone`) with retry-vs-drop per design §4, and refresh the destination after a successful move; verify a test where the fake rejects one op with `Protocol`: error shown, mailbox resynced, later op still delivered
- [x] 3.4 Trigger `drain` after each successful mailbox list; verify a test where the fake is offline for a flag change, comes back, and the change reaches the server after `SyncAll` without a toast while offline
- [x] 3.5 Verify ordering with a test: flag then move while offline; on reconnect the fake's log shows the flag before the move, and read-then-unread ends unread

## 4. Keeping the local view consistent (tern-sync)

- [x] 4.1 Re-apply queued ops touching a mailbox after each `apply_delta` (design §6); verify a test where a queued unread survives an incremental sync reporting read, and a queued move's message doesn't reappear after a full resync
- [x] 4.2 Drop queued ops for a mailbox when a delta has `ids_invalidated`, with one toast; verify a test using a fake that reports `ids_invalidated`
- [x] 4.3 Clear an account's queue on `RemoveAccount`; verify a test that no queued op is sent after removal
- [x] 4.4 Verify restart persistence with a test: queue ops offline against a file-backed store, build a new engine on the same store, sync, and the ops reach the server

## 5. Integration and docs

- [x] 5.1 Run `cargo test --workspace`, `cargo clippy --workspace --all-targets` (warning-free), `cargo machete`, and launch `--demo` to flag/archive/delete a message; verify all pass and the demo behaves as before
- [x] 5.2 Update `md/PLAN.md` (feature table "Offline queue" row and milestone 2 checklist) and add a `md/DEBUG.md` entry for the IMAP error-classification fix; verify the files describe the shipped behavior

## Workflow follow-up

- Archive the change with `/opsx:archive` after review, which turns the delta into `openspec/specs/mail-actions/spec.md`.
