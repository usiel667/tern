# Tern — notes for Claude Code

Apple Mail–style mail client for Linux. Rust 2024, GTK4 0.11 + libadwaita 0.9 (features
`v4_14` / `v1_5`), WebKitGTK 6. The owner develops on Arch + Hyprland with LazyVim and tmux.
`PLAN.md` has the feature map and roadmap; check it before starting a feature and update
its status table when you finish one.

## Layout

| Crate | Role | GTK? |
|---|---|---|
| `tern-core` | model, `MailBackend` trait, MIME parse, threading, search syntax, reply/forward | no |
| `tern-store` | SQLite cache + FTS5; `ListScope`; schema in `src/schema.sql` | no |
| `tern-imap` / `tern-jmap` | backends implementing `MailBackend` | no |
| `tern-smtp` | build RFC 5322 from `Draft`, SMTP submit | no |
| `tern-auth` | keyring secrets, Google OAuth2 | no |
| `tern-sync` | `Engine` (Tokio thread), `Command`/`Event`, demo backend | no |
| `tern` | the app: `app.rs` (state, actions, event loop), `sidebar.rs`, `message_list.rs`, `viewer.rs`, `compose.rs`, `accounts.rs`, `format.rs` (pure helpers) | yes |

## Rules of the road

- **Only the engine writes the store.** The UI holds a read connection (`Ui.store`) and
  changes things by sending `Command`s. New UI features = new `Command` + engine handler
  + `Event` the UI reacts to.
- **Engine store writes happen on its loop task.** Network work goes in `spawn_task`, and
  results come back as `Internal` messages. Every `spawn_task` future must return exactly
  one `Internal` (use `Internal::Nothing` when there's nothing to report) — `Done` is
  sent automatically and drives the busy spinner.
- **Optimistic UI.** Apply locally, emit `MessagesChanged` + `MailboxesChanged`, then push;
  on failure the engine resyncs that mailbox.
- **Never put secrets in `accounts.toml`, logs, or `Debug` output** (`Credential` redacts).
- **Message HTML is hostile.** Keep JavaScript off and the CSP in `format::render_body`.
- Keep GTK out of every crate except `tern`. Logic that can be pure goes in `tern-core`
  or `tern/src/format.rs` with unit tests.
- IMAP flags: only touch `MANAGED_FLAGS` with `+FLAGS`/`-FLAGS`; never `FLAGS` (it would
  wipe keywords other clients set).

## Commands

```sh
cargo run -- --demo                     # UI with fake data, no network/keyring
cargo test --workspace                  # must pass
cargo clippy --workspace --all-targets  # must be warning-free
TERN_IMAP_HOST=… TERN_IMAP_USER=… TERN_IMAP_PASS=… \
  cargo test -p tern-imap --test live -- --ignored --nocapture
RUST_LOG=tern_sync=debug,tern_imap=debug cargo run   # verbose sync logging
```

Testing the UI headless (what was used during development):

```sh
Xvfb :99 -screen 0 1400x880x24 &
DISPLAY=:99 GSK_RENDERER=cairo GTK_A11Y=none dbus-run-session -- ./target/debug/tern --demo &
DISPLAY=:99 xdotool mousemove 450 130 click 1
DISPLAY=:99 xwd -root -silent | convert xwd:- shot.png
```

Note: `xdotool windowkill` kills the whole app (all windows share one X client), and
synthetic `xdotool key --window` events are ignored by GTK4; click real coordinates instead.

## Conventions

- Errors: `tern_core::Error` variants; `Network`/`Protocol` make `ImapBackend` drop the
  connection so the next call reconnects. `Auth` makes the engine rebuild the backend.
- Icons: use names that exist in current Adwaita, or add a symbolic SVG under
  `crates/tern/data/icons` and list it in `data/resources.gresource.xml`.
- Shortcuts: Ctrl-combos go in `install_actions` accels; single-letter keys only in the
  message list's key controller (so they never fire while typing).
- Store schema change: bump `SCHEMA_VERSION` in `tern-store` and add a migration step.
