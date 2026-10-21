# Design

## Decisions

- **Draft identity = a key the compose window owns** (random UUID per window). The engine
  maps key → latest local version + where its server copy lives (`drafts` row:
  `key, account, raw, message_id, updated, uploaded, remote_mailbox, remote_id`).
- **Store the built RFC 5322 bytes**, not a serialized `Draft`: the same bytes are uploaded,
  and `draft_from_raw` already rebuilds a `Draft` (Bcc now read from the header, since
  drafts keep it). Each version gets a new Message-ID so versions are distinguishable on
  the server.
- **Upload = APPEND new, then Destroy old.** Never replace in place (IMAP can't). The old
  copy's delete goes through the existing durable `pending_ops` queue (`Op::Destroy`), so
  it survives going offline mid-way. The local message row of the old copy is removed at
  once so the list doesn't show two.
- **Knowing the new copy's id:** JMAP `Email/import` returns it. IMAP APPEND's APPENDUID
  isn't exposed by async-imap, so after APPEND, `UID SEARCH HEADER Message-ID` finds it
  (works on Gmail, iCloud, Dovecot). If that fails, the id stays unknown and the next
  replacement looks the old copy up by Message-ID in the local cache after a Drafts sync;
  worst case one stale copy remains (logged).
- **One upload per draft at a time.** A save during an upload marks the row not-uploaded;
  when the upload finishes, the newer version goes up and replaces the one just uploaded.
- **Autosave cadence**: debounce 4 s after the last change, plus a 30 s tick while dirty.
  Saves are skipped when nothing changed or the message has no content (recipients, subject,
  attachments, or body text other than the signature).
- **Send consumes the draft**: `Command::Send { draft_key }` discards the draft after the
  message is in the outbox (the outbox is now the durable copy).
- **Editing a server draft** passes `replaces: MessageId` on the first save, so the
  engine adopts that message as the draft's server copy and replaces it.

## Risks / Trade-offs

- Gmail keeps deleted drafts in Trash/All Mail like any IMAP delete; that's Gmail's
  behaviour for every IMAP client.
- Uploading every 30 s while typing a long message with big attachments costs bandwidth;
  attachments are re-sent with each version. Acceptable for now; a later change could only
  upload on pause/close when attachments are large.
