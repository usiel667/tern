# Tasks

- [x] 1.1 `folder_zip::zip_folder(dir, max)` (hidden/symlinks skipped, stored vs deflated, permissions, times, size cap); verify unit tests: tree + hidden + symlink + contents + methods, empty folder, over the cap
- [x] 1.2 Compose: directories zipped via `gio::spawn_blocking`, "Compressing…" chips, `do_send` guard; verify in the test display with the real Dolphin: `Project` → `Project.zip`, `Huge` (120 MB) shows the spinner chip then `Huge.zip` and the 25 MB warning, Ctrl+Return while compressing shows the notice
- [x] 1.3 `cargo test --workspace`, clippy warning-free, `cargo audit`, `cargo machete`
- [x] 1.4 `md/PLAN.md`, `md/UI.md`
