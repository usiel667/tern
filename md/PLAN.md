# Tern plan

Goal: match Apple Mail's behaviour as closely as practical on Linux.
Status keys: ✅ done · 🟡 partial · ⬜ not started.

## Architecture

```
            ┌──────────── tern (GTK4 + libadwaita UI, main thread) ───────────┐
            │ sidebar · message_list · viewer (WebKitGTK) · compose · accounts │
            └───────▲──────────────────────────────┬─────────────────────────────┘
       read-only    │ Event (async-channel)        │ Command (async-channel)
       SQLite conn  │                              ▼
┌───────────────┐   │   ┌──────────── tern-sync (Tokio thread) ─────────────┐
│  tern-store   │◄──┴───│ Engine: only writer to the store; spawns network   │
│ SQLite + FTS5 │◄──────│ tasks, applies results on one task; outbox; IDLE   │
└───────────────┘       └──────┬───────────────┬───────────────┬─────────────┘
                               ▼               ▼               ▼
                        tern-imap        tern-jmap        tern-smtp
                     (async-imap,     (reqwest, RFC     (lettre; JMAP
                      rustls)          8620/8621)        submits itself)
                               └──── implement tern-core::MailBackend ────┘
tern-core: model, MailBackend trait, MIME parsing, threading, search syntax, reply/forward
tern-auth: Secret Service (keyring) + Google OAuth2 PKCE loopback
```

Key decisions:

- **Local-first.** The UI only ever reads SQLite. Opening the app is instant and
  everything works offline; the engine reconciles in the background.
- **Optimistic writes.** Flag/move/delete update the store immediately, then push to
  the server; failures trigger a resync of that mailbox (rollback).
- **One protocol-neutral trait.** IMAP and JMAP look identical to the engine.
- **Outbox first.** Sends are written to `outbox` before submission, so nothing is
  lost if Tern quits or the network drops; flushed at startup and every 5 min.
- **Conversations are computed locally** (union-find over Message-ID/References plus
  JWZ subject merge) across all mailboxes, so your Sent replies join threads.
- **Mail HTML is untrusted.** WebKit with JavaScript off, a strict CSP that blocks all
  remote loads until "Load Remote Content", `cid:` images inlined as `data:` URIs,
  link clicks open in the browser.

## Apple Mail feature map

### Accounts & sync
| Feature | Status | Notes |
|---|---|---|
| iCloud / Google / Fastmail / generic IMAP presets | ✅ | `AccountConfig::preset` |
| Verify credentials before saving | ✅ | Add Account sheet logs in once |
| Google OAuth2 | ✅ | needs user's own client id (README) |
| Secrets in system keyring | ✅ | Secret Service via `keyring` |
| Initial sync of recent mail per mailbox | ✅ | 1000 newest (`INITIAL_SYNC_LIMIT`) |
| Incremental sync, flag changes, expunges | ✅ | UID window; live-tested on Dovecot |
| Push (IMAP IDLE on inbox) | ✅ | reconnect with backoff |
| JMAP push (EventSource) | ⬜ | polls every 2 min for now |
| CONDSTORE / QRESYNC | ✅ | only changed flags / VANISHED; idle QRESYNC mailbox costs nothing; spec: `openspec` `mailbox-sync` |
| Gmail labels (X-GM-LABELS), hide All Mail dupes | ⬜ | labels currently appear as folders |
| Offline queue for flag/move ops | ✅ | every action queued in `pending_ops`, delivered in order; spec: `openspec` `mail-actions` |
| Download older mail on scroll | ✅ | 250 per batch at the bottom of a mailbox or unified view; spec: `openspec` `mail-history` |
| Multiple identities / aliases per account | ⬜ | |
| Exchange / EWS | ⬜ | out of scope for now |

### Reading
| Feature | Status | Notes |
|---|---|---|
| Three-pane layout, collapsible on narrow tiles | ✅ | adaptive breakpoints |
| Favorites: All Inboxes, Flagged, Drafts, Sent | ✅ | |
| Smart mailbox: Unread | ✅ | |
| User-defined Smart Mailboxes (rules editor) | ⬜ | `ListScope` + SQL filters make this straightforward |
| VIPs | ⬜ | |
| Organize by Conversation | ✅ | toggle in menu; count badge; replies only (Gmail-style same-subject grouping planned as a setting) |
| Conversation view showing all messages | 🟡 | list of thread messages above the open one; Apple stacks full bodies |
| Unread dot, colored flags (7 colors), replied/forwarded icons | ✅ | flags round-trip via `$MailFlagBit*` like Apple |
| Two-line previews, Apple-style dates | ✅ | |
| HTML mail, remote content blocking | ✅ | |
| Inline (cid:) images | ✅ | |
| Attachments: list, save | ✅ | |
| Quick Look / open with default app | ⬜ | |
| List-Unsubscribe banner | 🟡 | opens link/mailto; RFC 8058 one-click POST TODO |
| Mark read on open | ✅ | |
| Mail Privacy Protection (proxy remote content) | ⬜ | blocking already prevents tracking |
| Open message in its own window (double-click) | ⬜ | |
| View source / raw headers | ⬜ | raw bytes are stored |
| Print | ⬜ | WebKit print operation |
| Phishing/junk detection | ⬜ | rely on server for now |
| Desktop notifications | ✅ | click opens the message |
| Unread count in title | ✅ | |

### Organizing
| Feature | Status | Notes |
|---|---|---|
| Delete → Trash, delete in Trash → permanent | ✅ | |
| Archive (Archive or All Mail) | ✅ | |
| Junk / Not Junk | ✅ | |
| Multi-select actions | ✅ | conversation rows expand to their messages |
| Drag messages to a mailbox | ✅ | |
| Right-click menu | ✅ | |
| Swipe gestures | ⬜ | touchpad swipe-to-archive |
| Create / rename / delete mailboxes | ⬜ | |
| Rules | ⬜ | |
| Undo (Ctrl+Z) for move/delete | ⬜ | keep inverse op for ~10s |
| Follow-up suggestions, Remind Me | ⬜ | |

### Search
| Feature | Status | Notes |
|---|---|---|
| Full-text over subject/from/to/preview/body | ✅ | FTS5, prefix match, diacritic-folded |
| Search tokens (from:, subject:, is:, has:, dates) | ✅ | text syntax; Apple-style token chips ⬜ |
| Scope: current mailbox vs everything | 🟡 | searches current scope only |
| Server-side search for un-cached mail | ⬜ | |

### Composing
| Feature | Status | Notes |
|---|---|---|
| New / Reply / Reply All / Forward | ✅ | Apple attribution + quoting rules |
| From account picker | ✅ | |
| Cc / Bcc | ✅ | Bcc kept out of headers |
| Attachments (pick) | ✅ | forward carries originals |
| Attachments (drag & drop into window) | ✅ | anywhere on the window incl. body; folders zipped (`Name.zip`, no hidden files/symlinks, ≤ 200 MB); links become text; sizes on chips, 25 MB warning; Shift+Ctrl+A picker |
| Signatures | 🟡 | per-account plain text in accounts.toml; no editor yet |
| Address autocomplete from history/contacts | ✅ | from cached mail (`contacts` table, schema v3); people you wrote to rank first; no system address book yet |
| Rich text (bold, lists, fonts) | ⬜ | likely a WebKit contenteditable editor |
| Save to Drafts / autosave | ✅ | local first (schema v5), then APPEND to Drafts replacing the previous copy; 4 s pause / 30 s while typing, Ctrl+S; close asks Save / Don't Save / Cancel; Edit Draft banner + Return/double-click; Bcc kept |
| Undo Send | ✅ | held 10 s in the outbox (engine timer); Undo reopens the same window and re-saves the draft; a hold interrupted by quitting sends on next start; delay setting comes with Settings |
| Send later | ⬜ | scheduled send; the hold mechanism is a start |
| Sent copy filed in Sent | ✅ | skipped for Gmail (server does it) |
| Server size limits | ✅ | SMTP `SIZE` / JMAP `maxSizeUpload` per account; exact check on Send, estimate while attaching |
| Refused sends (Unsent Messages) | ✅ | not retried; Edit / Try Again / Delete; JMAP `notCreated` no longer counts as sent |
| mailto: handler | ✅ | in-message links, `tern mailto:…` (single instance), .desktop MimeType; RFC 6068 fields only, `attach` ignored |

### Settings
| Feature | Status | Notes |
|---|---|---|
| Preferences window | ⬜ | accounts list, signatures, fetch interval, remote content default, notification scope |
| Remove account | 🟡 | engine supports it; no UI |
| Vim key bindings (optional) | ⬜ | owner request 2026-10-21: toggle in Preferences; move around the whole app (j/k messages, h/l between panes, gg/G, / search, …) without breaking typing in text fields |
| Themes | ⬜ | owner request 2026-10-21: Preferences picker — System/Light/Dark plus Tokyo Night (Night, Storm, Moon, Day), Catppuccin (Mocha, Macchiato, Frappé, Latte), Rosé Pine (Main, Moon, Dawn); likely also Gruvbox, Nord, Dracula, Everforest, Kanagawa. Palettes are MIT-licensed (fine under GPL, keep attribution). Must also tint the message viewer and compose, not just the chrome |

## Milestones

1. **Read + compose + send** — done (this commit).
2. **Robustness** (in progress):
   - ✅ Outbox: store the SMTP envelope with each row (Bcc survives a resumed send),
     never dispatch a row that's already sending, record failed attempts (schema v2).
   - ✅ Rate-limit auth retries: first reconnect immediate, then 30 s doubling to 30 min;
     one reconnect at a time per account; failed rebuilds (OAuth refresh offline) retry too.
   - ✅ Replay `pending_ops`: every flag/move/delete is queued and delivered in order per
     account; network/auth failures keep it, server rejections roll back; syncs don't revert
     queued changes (OpenSpec change `replay-offline-actions`).
   - ✅ Load older mail on scroll: list pages through the cache 500 rows at a time, then
     `LoadOlder` fetches 250 older per mailbox (IMAP UID SEARCH, JMAP anchored query);
     shares the per-mailbox sync slot (OpenSpec change `load-older-mail`).
   - ✅ CONDSTORE/QRESYNC: cursor v2 stores HIGHESTMODSEQ; QRESYNC → CHANGEDSINCE + VANISHED,
     CONDSTORE → CHANGEDSINCE + UID SEARCH, neither → full check; full check every 50th sync.
     Live-tested against the local Dovecot in all three modes (OpenSpec change `imap-condstore-qresync`).
   - ⬜ JMAP EventSource push.
3. **Compose polish** (in progress):
   - ✅ mailto: handler: `tern mailto:…` opens in the running instance (GApplication
     `HANDLES_OPEN`), desktop file registers `x-scheme-handler/mailto`, RFC 6068 parser
     (literal `+`, repeated recipients, no `attach`); a link's body goes above the signature
     (OpenSpec change `mailto-handler`).
   - ✅ Address autocomplete: the engine learns addresses as mail is stored (Sent → To/Cc,
     elsewhere → From; Drafts/Junk/Trash and noreply-style senders ignored), schema v3
     backfills cached mail; To/Cc/Bcc show up to 8 suggestions (OpenSpec change `address-autocomplete`).
   - ✅ Drag-and-drop attachments: capture-phase drop target on the compose window (beats the
     body's text drop), files read async (picker too), folders skipped, browser links inserted
     as text, chip sizes + 25 MB warning, Shift+Ctrl+A opens the picker (OpenSpec change
     `drag-drop-attachments`). Dropped folders are zipped in the background and attached as
     `Name.zip`; sending waits until compression finishes (`zip-dropped-folders`).
   - ✅ Send size limits and refused sends: each account's real limit (Gmail 35,882,577,
     iCloud 28,319,744 bytes encoded), checked on Send; server refusals stop retrying and go to
     Unsent Messages (schema v4) (OpenSpec change `send-size-limits`).
   - ✅ Gmail app passwords: Add Account → Google asks for an app password (spaces removed);
     browser sign-in is offered only when an OAuth client is configured; a refused password
     explains app passwords (OpenSpec change `gmail-app-passwords`).
   - ✅ Save to Drafts / autosave: drafts are saved on this computer first, then uploaded to
     the account's Drafts mailbox replacing the previous copy (IMAP finds the new UID by
     Message-ID); sending or Don't Save removes them; drafts from other apps open for editing
     (OpenSpec change `drafts-autosave`).
   - ✅ Undo Send: Send waits 10 seconds with an Undo toast; Undo brings the window back as
     it was (OpenSpec change `undo-send`).
   - ⬜ Signature editor.
4. **Organizing:** mailbox management, Smart Mailbox editor, VIPs, Undo, rules.
5. **Reading polish:** full stacked conversation view, open-in-window, print, view source,
   RFC 8058 one-click unsubscribe, Quick Look via xdg-open.
6. **Settings + packaging:** Preferences window (incl. optional Vim key bindings, themes,
   the Undo Send delay: Off / 10 / 20 / 30 s, and conversation grouping: replies only (default,
   today's behaviour) or also Gmail-style — same exact subject from the same sender within
   ~3 days, decided 2026-10-22),
   AUR package, Flatpak manifest.

## Known gaps worth knowing now

- First launch syncs every mailbox's newest 1000 messages (older mail loads as you scroll);
  unread/total badges still show the server's full counts.
- A sync round on a big Gmail account takes 9–17 s even with nothing new (18 mailboxes, one connection); opening messages no longer waits for it, but new mail and flag changes do.
- The Flagged badge counts only downloaded mail (IMAP has no cheap server-wide flagged count).
- IMAP servers without CONDSTORE still fetch FLAGS for the whole cached UID window on every
  sync, so loading a lot of older mail makes their syncs heavier.
- "Nothing older" is remembered per session only; after a restart the first scroll to the end asks again.
- Gmail shows labels as folders (as Apple Mail does over IMAP), so a message can appear in several.
- Sends that fail for temporary reasons (offline, server busy) retry every 5 minutes with no
  limit and aren't listed anywhere; only messages a server *refused* show in Unsent Messages.
- A server that only reveals its size limit after sign-in reports none to Tern; such
  messages are still caught when the server refuses them.
- Sends queued before schema v2 (milestone 1) still lose Bcc if resumed.
- Each autosave re-uploads the whole draft, attachments included; drafts with big
  attachments use bandwidth every 30 s while you type.
- Accounts without a Drafts mailbox keep drafts on this computer only, with no list of them.
- Address suggestions can't be removed or edited, and Bcc recipients are only learned if
  they write back (they're not in the Sent copy).
- The UI layer has no automated tests yet; `--demo` plus Xvfb screenshots is the current check.
