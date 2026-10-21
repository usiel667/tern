# Tasks

## 1. Parser

- [x] 1.1 Replace `open_mailto`'s parsing with a pure `mailto_draft` (RFC 6068 whitelist, literal `+`, CRLF → LF, repeated recipients); verify with unit tests for plus addressing, repeated/uppercase fields, multi-line body, `attach` ignored, missing prefix

## 2. Single-instance open

- [x] 2.1 `app::build` returns `Option<Rc<Ui>>`; `main.rs` sets `HANDLES_OPEN`, shares the `Ui` between `activate` and `open`, opens a compose window per `mailto:` file and toasts for anything else; verify `tern --demo mailto:…` opens a filled compose window both cold and with the demo already running
- [x] 2.2 Desktop file: `Exec=tern %u`, `MimeType=x-scheme-handler/mailto;`; verify `desktop-file-validate`

## 3. Integration and docs

- [x] 3.1 `cargo test --workspace`, `cargo clippy --workspace --all-targets` warning-free; verify both pass
- [x] 3.2 Update `md/PLAN.md` (mailto row, milestone 3 checklist) and README if it documents install/usage; verify they describe the shipped behavior

## Workflow follow-up

- Archive the change with `/opsx:archive` after review, creating `openspec/specs/compose/spec.md`.
