# Tasks

## 1. Helpers (tern/format.rs)

- [x] 1.1 `attachment_label(name, size)` and `too_large(total)` (25 MB); verify unit tests

## 2. Compose (tern/compose.rs)

- [x] 2.1 Shared async `add_files` (native files via `load_contents_future`, folders skipped with a toast, errors per file, size warning once the total crosses 25 MB); switch the picker to it; verify the picker still attaches in the test display
- [x] 2.2 Capture-phase `DropTarget` for `FileList` on the window, non-native URIs inserted as body text, `drop-hover` class + "Drop to attach" hint on enter/leave; verify by hand with a file manager (two files on the body, a folder, a browser link) — PENDING: needs the owner to try it; not automatable — done with the real Dolphin in the test display (`pointer drag`): file on body, folder, hover; browser link not verified (no browser in the test display)
- [x] 2.3 Chips show sizes; verify in the test display

## 3. Integration and docs

- [x] 3.1 `cargo test --workspace`, `cargo clippy --workspace --all-targets` warning-free
- [x] 3.2 `md/PLAN.md` row + milestone 3 checklist, `md/UI.md` Done entry
