# Proposal

## Why

Every recipient in Tern has to be typed in full. Apple Mail suggests addresses as you type,
drawn from people you've written to ("Previous Recipients") and your contacts. Tern already
has every synced message's From/To/Cc in its cache, so it can do the same without any
address book. This is milestone 3's "address autocomplete".

## What Changes

- The store keeps a `contacts` table: one row per email address (case-insensitive) with the
  latest display name, how often you've written to it, how often it wrote to you, and when it
  was last seen. The engine updates it as it stores new messages:
  - messages in a **Sent** mailbox count their To and Cc as people you wrote to;
  - messages in **Drafts, Junk and Trash** count for nothing (so spammers aren't suggested);
  - messages anywhere else count their From as someone who wrote to you.
- Machine senders (`noreply`, `no-reply`, `donotreply`, `mailer-daemon`, `postmaster`,
  `bounce…`) are never recorded.
- Schema v3 adds the table; the migration fills it from mail already cached.
- Typing in To, Cc or Bcc shows up to 8 suggestions under the field for the address being
  typed (the text after the last comma), matching the start of the email or of any word of
  the name. People you've written to come first, then by how often and how recently.
  Your own addresses and addresses already in that field aren't suggested.
- Up/Down move through suggestions, Return or Tab picks one (the field gets
  `Name <email>, ` and the cursor goes to the end), Escape closes the list without closing
  the window, clicking a suggestion picks it.

Out of scope: system address books (GNOME Contacts/EDS, CardDAV), forgetting or editing a
suggestion, recipient "chips", recording Bcc recipients at send time (they're not in the
Sent copy, so they're only learned if they reply).

## Capabilities

### New Capabilities

(none)

### Modified Capabilities
- `compose`: adds recipient suggestions.

## Impact

- `tern-store`: `contacts` table, schema v3 + migration with backfill, recording in
  `apply_delta` for newly inserted rows only, `Store::suggest_addresses(prefix, limit)`.
- `tern` (UI): a suggestion popover attached to the To/Cc/Bcc entries in `compose.rs`.
- No engine or protocol changes; the engine already owns all store writes.
