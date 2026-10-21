# Proposal

## Why

Tern opens `mailto:` links inside messages, but nothing outside Tern can hand it one: the
desktop file doesn't register for `x-scheme-handler/mailto` and `tern mailto:…` on the
command line is ignored. So "Email this" links in a browser, or `xdg-open mailto:…`, go to some
other mail app. The mailto parser also reads `+` as a space, which breaks addresses like
`me+tag@example.com`. This is milestone 3's "mailto: CLI handler".

## What Changes

- `tern mailto:…` opens a New Message window filled from the link. If Tern is already
  running, the running instance opens it (no second engine); otherwise Tern starts normally
  and then opens it.
- The desktop file declares `MimeType=x-scheme-handler/mailto;` and `Exec=tern %u`, so Tern
  can be chosen as the default mail app (`xdg-mime default dev.tern.Tern.desktop x-scheme-handler/mailto`).
- The parser follows RFC 6068: `to`, `cc`, `bcc`, `subject`, `body` and `in-reply-to` are
  read (names case-insensitive, repeated `to`/`cc`/`bcc` add up), `+` stays a literal plus,
  `%0D%0A` line breaks become newlines. Every other header, including `attach`/`attachment`,
  is ignored so a web page can't attach local files or set arbitrary headers.
- Anything that isn't a `mailto:` URI passed on the command line shows a toast instead of
  being silently dropped.

Out of scope: a Preferences toggle for "make Tern the default mail app" (milestone 6) and
`--compose`-style flags.

## Capabilities

### New Capabilities
- `compose`: how new messages are started and filled in (starting with `mailto:` links).

### Modified Capabilities

(none)

## Impact

- `tern` (UI): `main.rs` gets `HANDLES_OPEN` and an `open` handler; `app::build` returns the
  `Ui` so both handlers share it; `compose.rs` parser rewritten as a pure, tested function.
- `crates/tern/data/dev.tern.Tern.desktop`: `Exec` and `MimeType`.
