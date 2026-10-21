# Design

## Context

`main.rs` builds the window in `connect_activate` and keeps nothing; `compose::open_mailto`
needs the `Rc<Ui>`. GApplication already makes a second `tern` invocation forward its
arguments to the primary instance over D-Bus, so a single-instance handler is mostly wiring.

## Decisions

- **`HANDLES_OPEN`, not `HANDLES_COMMAND_LINE`.** With `HANDLES_OPEN`, GIO turns positional
  arguments into `GFile`s (a `mailto:` argument becomes a file whose `uri()` is the original
  string) and calls `open` in the primary instance. That's exactly the shape we need and
  keeps our own `--demo`/`--profile` parsing as is. Command-line forwarding would mean
  re-parsing in the primary.
- **One shared `Rc<RefCell<Option<Rc<Ui>>>>`** captured by both `activate` and `open`.
  `open` builds the UI first when it doesn't exist yet, then opens one compose window per
  `mailto:` argument. `build` returns `Option<Rc<Ui>>` (None when startup failed and the
  error dialog is shown).
- **Pure parser `mailto_draft(&str) -> Draft`** with or without the `mailto:` prefix, so the
  viewer's in-message links and the CLI go through the same code and it's unit-testable.
  Header whitelist (to, cc, bcc, subject, body, in-reply-to); `+` is literal per RFC 6068;
  CRLF → LF in the body; percent-decoding is done on bytes and then UTF-8 (lossy).
- **The demo app ID differs** (`dev.tern.Tern.Demo`), so `tern --demo mailto:…` talks to a
  running demo, not the real instance. Fine; no change.

## Risks / Trade-offs

- `attach` is deliberately unsupported. Some clients accept it; it's a known exfiltration
  trick (a link attaching `~/.ssh/id_rsa`), so ignoring it is the safe default.
- If the main window was closed while a compose window kept the app alive, `open` presents
  the existing (hidden) main window again rather than building a second UI.
