# Proposal

## Why

Tern downloads only the newest 1,000 messages of each mailbox and never fetches anything
older, so mail from before that point can't be read or found in a local search. The list also
stops at 2,000 rows even when more is cached. Apple Mail fetches older messages as you scroll
down; Tern should too. This is milestone 2's "load older mail on scroll".

## What Changes

- Scrolling to the bottom of a mailbox's message list first shows more of what is already
  cached, and once everything cached is shown, downloads the next batch of older messages
  from the server.
- Older messages are fetched in batches (250 per mailbox per request), newest-first, and kept
  like any other synced message: their flags stay up to date and they're searchable.
- When the server has nothing older, the mailbox stops asking until the next app start.
- Unified views that map to mailboxes ("All Inboxes" and the other per-role favourites) load
  older mail for every mailbox they include.
- The demo account gets a few hundred generated older messages in its inbox so the
  behavior can be tried without a real account.

Assumptions (minor, recorded rather than asked):
- Batch size 250; the list grows by 500 rows per bottom-of-list scroll.
- Smart views that aren't tied to mailboxes (Flagged, Unread, search results) show what's
  cached but don't trigger downloads; server-side search is a separate PLAN item.

Out of scope: a "Load more" button or progress footer (the existing busy spinner is used),
remembering "nothing older" across restarts, and making the initial 1,000-message download
configurable.

## Capabilities

### New Capabilities
- `mail-history`: how much of each mailbox is kept locally and how older mail is shown and
  downloaded on demand.

### Modified Capabilities

(none; `mail-actions` is unaffected)

## Impact

- `tern-core`: new `MailBackend` method to fetch messages older than the cached window, with
  a default so test fakes need no change.
- `tern-imap`: implementation using UID SEARCH below the cached window's lowest UID; the
  cursor's low UID moves down, so later syncs keep the older messages' flags current.
- `tern-jmap`: implementation using `Email/query` anchored at the oldest cached message.
- `tern-sync`: new `Command` to load older mail for a set of mailboxes; serialized with
  regular syncs of the same mailbox so cursor updates can't race.
- `tern` (UI): bottom-of-list detection, growing list limit, sending the new command.
- `tern-sync` demo backend: generated older inbox history behind a moving window.
