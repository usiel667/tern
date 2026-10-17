# Tern — notes for Claude Code

Apple Mail–style mail client for Linux. Rust 2024, GTK4 0.11 + libadwaita 0.9 (features
`v4_14` / `v1_5`), WebKitGTK 6. The owner develops on Arch + Hyprland with LazyVim and tmux.
`md/PLAN.md` has the feature map and roadmap; check it before starting a feature and update
its status table when you finish one.

Project notes live in `md/` (only `README.md` and this file stay at the root). Log bugs
and investigations in `md/DEBUG.md` (newest first) and look/feel requests and decisions
in `md/UI.md`; add to them as you go.

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

<!-- gitnexus:start -->
# GitNexus — Code Intelligence

This project is indexed by GitNexus as **tern** (1035 symbols, 2247 relationships, 89 execution flows). Use the GitNexus MCP tools to understand code, assess impact, and navigate safely.

> If any GitNexus tool warns the index is stale, run `npx gitnexus analyze` in terminal first.

## Always Do

- **MUST run impact analysis before editing any symbol.** Before modifying a function, class, or method, run `gitnexus_impact({target: "symbolName", direction: "upstream"})` and report the blast radius (direct callers, affected processes, risk level) to the user.
- **MUST run `gitnexus_detect_changes()` before committing** to verify your changes only affect expected symbols and execution flows.
- **MUST warn the user** if impact analysis returns HIGH or CRITICAL risk before proceeding with edits.
- When exploring unfamiliar code, use `gitnexus_query({query: "concept"})` to find execution flows instead of grepping. It returns process-grouped results ranked by relevance.
- When you need full context on a specific symbol — callers, callees, which execution flows it participates in — use `gitnexus_context({name: "symbolName"})`.

## Never Do

- NEVER edit a function, class, or method without first running `gitnexus_impact` on it.
- NEVER ignore HIGH or CRITICAL risk warnings from impact analysis.
- NEVER rename symbols with find-and-replace — use `gitnexus_rename` which understands the call graph.
- NEVER commit changes without running `gitnexus_detect_changes()` to check affected scope.

## Resources

| Resource | Use for |
|----------|---------|
| `gitnexus://repo/tern/context` | Codebase overview, check index freshness |
| `gitnexus://repo/tern/clusters` | All functional areas |
| `gitnexus://repo/tern/processes` | All execution flows |
| `gitnexus://repo/tern/process/{name}` | Step-by-step execution trace |

## CLI

| Task | Read this skill file |
|------|---------------------|
| Understand architecture / "How does X work?" | `.claude/skills/gitnexus/gitnexus-exploring/SKILL.md` |
| Blast radius / "What breaks if I change X?" | `.claude/skills/gitnexus/gitnexus-impact-analysis/SKILL.md` |
| Trace bugs / "Why is X failing?" | `.claude/skills/gitnexus/gitnexus-debugging/SKILL.md` |
| Rename / extract / split / refactor | `.claude/skills/gitnexus/gitnexus-refactoring/SKILL.md` |
| Tools, resources, schema reference | `.claude/skills/gitnexus/gitnexus-guide/SKILL.md` |
| Index, status, clean, wiki CLI commands | `.claude/skills/gitnexus/gitnexus-cli/SKILL.md` |

<!-- gitnexus:end -->
