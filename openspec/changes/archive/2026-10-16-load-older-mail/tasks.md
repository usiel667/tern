# Tasks

## 1. Backend API (tern-core)

- [x] 1.1 Add `OlderFrom`, `OlderBatch` and `MailBackend::fetch_older` with a default that returns no older mail and the cursor unchanged; verify `cargo build --workspace --all-targets` passes with no changes to fakes

## 2. IMAP (tern-imap)

- [x] 2.1 Add a pure helper that picks the next `count` UIDs below `low_uid` from UID SEARCH results and reports `more`; verify with unit tests (fewer than count, exactly count, more than count, `low_uid <= 1`)
- [x] 2.2 Implement `fetch_older` (UID SEARCH, UID FETCH, lowered `low_uid`, UIDVALIDITY mismatch → empty batch with `more: true`); verify with `cargo build -p tern-imap` and a new ignored case in `tests/live.rs` for a real server

## 3. JMAP (tern-jmap)

- [x] 3.1 Implement `fetch_older` with `Email/query` anchored at the oldest cached id (`anchorOffset 1`, `calculateTotal`), falling back to `position` on `anchorNotFound`; verify with a unit test of the request and of `more` from a canned response

## 4. Engine (tern-sync)

- [x] 4.1 Add `Command::LoadOlder`, `Internal::Older`, the `exhausted` and `older_wanted` sets, and start loads in the shared per-mailbox `syncing` slot; verify a test with the fake: older messages are stored, the cursor's window grows, and queued actions still overlay them
- [x] 4.2 Verify serialization and de-duplication with tests: `LoadOlder` during a regular sync runs after it, a second `LoadOlder` while loading is ignored, and the older rows survive the next incremental sync
- [x] 4.3 Stop at the start of the mailbox and handle errors; verify tests: after a batch with `more: false` no further fetch happens, a network failure releases the slot with no toast, and a protocol failure toasts without a mailbox rollback
- [x] 4.4 Implement windowed history in the demo backend (generated older inbox mail, initial window of 50, `fetch_older`); verify with a test that drives the demo backend through the engine until exhausted

## 5. UI (tern)

- [x] 5.1 Add the growing list limit (500, +500 on bottom edge, reset on scope/search change) and send `LoadOlder` for Mailbox and Role scopes once all cached rows are shown; verify in `--demo` that scrolling the inbox loads older messages until they run out, Flagged/search trigger nothing, and the list keeps its selection across reloads

## 6. Integration and docs

- [x] 6.1 Run `cargo test --workspace`, `cargo clippy --workspace --all-targets` (warning-free) and `cargo machete`; verify all pass
- [x] 6.2 Update `md/PLAN.md` (feature table "Download older mail on scroll" and the milestone 2 checklist) and note the larger IMAP FLAGS window under known gaps until CONDSTORE; verify the files describe the shipped behavior

## Workflow follow-up

- Archive the change with `/opsx:archive` after review, creating `openspec/specs/mail-history/spec.md`.
