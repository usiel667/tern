# Tasks

## 1. Store (tern-store)

- [x] 1.1 Add the `contacts` table to `schema.sql`, bump `SCHEMA_VERSION` to 3, record contacts for newly inserted rows in `apply_delta` by mailbox role, skipping machine senders; verify tests: inbox From and Sent To/Cc recorded, Junk/Trash/Drafts ignored, re-applying the same message doesn't double-count, noreply skipped, name updates to the latest
- [x] 1.2 `suggest_addresses(prefix, limit)` with escaped `LIKE` on email and name words and the sent-first ranking; verify tests: name-word match, email-prefix match, `%`/`_` literal, ranking colleague above frequent sender
- [x] 1.3 v2→v3 migration backfills from cached messages; verify a test that builds a v2 database with messages, opens it, and gets suggestions

## 2. UI (tern)

- [x] 2.1 Suggestion popover for To/Cc/Bcc (current token after last comma, own and already-present addresses filtered, keys only while showing, click to pick, unparent on destroy); verify in `--demo`: typing a demo sender's name suggests them, Return/Tab/click fill the field, Escape closes only the list, Ctrl+Return still sends

## 3. Integration and docs

- [x] 3.1 `cargo test --workspace`, `cargo clippy --workspace --all-targets` warning-free
- [x] 3.2 Update `md/PLAN.md` (autocomplete row, milestone 3 checklist); verify it describes the shipped behavior

## Workflow follow-up

- Archive with `/opsx:archive` after review (adds requirements to `openspec/specs/compose/spec.md`).

Note: picking by mouse click was not exercised live (no pointer injection available); it uses the same `pick` path as Return/Tab.
