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
| CONDSTORE / QRESYNC | ⬜ | big win for large mailboxes |
| Gmail labels (X-GM-LABELS), hide All Mail dupes | ⬜ | labels currently appear as folders |
| Offline queue for flag/move ops | 🟡 | `pending_ops` table exists; not replayed yet |
| Download older mail on scroll | ⬜ | |
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
| Organize by Conversation | ✅ | toggle in menu; count badge |
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
| Attachments (drag & drop into window) | ⬜ | |
| Signatures | 🟡 | per-account plain text in accounts.toml; no editor yet |
| Address autocomplete from history/contacts | ⬜ | query distinct from/to in store |
| Rich text (bold, lists, fonts) | ⬜ | likely a WebKit contenteditable editor |
| Save to Drafts / autosave | ⬜ | APPEND to Drafts |
| Send later, Undo Send | ⬜ | outbox delay makes Undo Send easy |
| Sent copy filed in Sent | ✅ | skipped for Gmail (server does it) |
| mailto: handler | 🟡 | in-message links work; CLI arg + .desktop MimeType TODO |

### Settings
| Feature | Status | Notes |
|---|---|---|
| Preferences window | ⬜ | accounts list, signatures, fetch interval, remote content default, notification scope |
| Remove account | 🟡 | engine supports it; no UI |

## Milestones

1. **Read + compose + send** — done (this commit).
2. **Robustness** (in progress):
   - ✅ Outbox: store the SMTP envelope with each row (Bcc survives a resumed send),
     never dispatch a row that's already sending, record failed attempts (schema v2).
   - ✅ Rate-limit auth retries: first reconnect immediate, then 30 s doubling to 30 min;
     one reconnect at a time per account; failed rebuilds (OAuth refresh offline) retry too.
   - ⬜ Replay `pending_ops` (offline flag/move).
   - ⬜ Load older mail on scroll.
   - ⬜ CONDSTORE/QRESYNC.
   - ⬜ JMAP EventSource push.
3. **Compose polish:** drafts/autosave, address autocomplete, drag-and-drop attachments,
   Undo Send, signature editor, mailto: CLI handler.
4. **Organizing:** mailbox management, Smart Mailbox editor, VIPs, Undo, rules.
5. **Reading polish:** full stacked conversation view, open-in-window, print, view source,
   RFC 8058 one-click unsubscribe, Quick Look via xdg-open.
6. **Settings + packaging:** Preferences window, AUR package, Flatpak manifest.

## Known gaps worth knowing now

- First launch syncs every mailbox's newest 1000 messages; very large accounts will take a while.
- Gmail shows labels as folders (as Apple Mail does over IMAP), so a message can appear in several.
- Failed sends retry every 5 minutes with no limit, and there's no Outbox view to see or
  cancel them (attempts and last error are recorded in the `outbox` table).
- Sends queued before schema v2 (milestone 1) still lose Bcc if resumed.
- The UI layer has no automated tests yet; `--demo` plus Xvfb screenshots is the current check.
