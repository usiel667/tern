# Design

## Decisions

- **Probe SIZE separately, without credentials.** `lettre` parses EHLO into known
  extensions only and drops `SIZE`. Its low-level `AsyncSmtpConnection` returns the raw
  EHLO reply, so `tern_smtp::size_limit` connects (implicit TLS or STARTTLS), sends EHLO,
  parses `SIZE n` (`0` = no limit) and quits. Most servers (Gmail included) advertise
  `SIZE` before AUTH. Run when an account becomes ready; the engine keeps the result in
  memory and emits `Event::SendLimit`.
- **The limit is for the whole encoded message.** SMTP `SIZE` and JMAP `maxSizeUpload`
  both bound the RFC 5322 bytes Tern builds, so the Send check compares `build(draft).raw`
  — exact, not an estimate. The attach-time warning estimates (base64 ≈ ×4/3 plus line
  breaks) since it only advises.
- **`Error::Rejected` is the single "don't retry" signal** for sends. Classification lives
  where the protocol knowledge is: SMTP reply codes in `tern-smtp`, JMAP set-errors in
  `tern-jmap`. The engine only checks the variant.
- **Mark, don't delete.** `outbox.rejected = 1` keeps the raw message and envelope (the only
  copy of Bcc) until the user edits, retries or deletes it. `Store::outbox()` returns only
  unrejected rows, so `flush_outbox` never retries them.
- **Edit = rebuild a Draft from the raw message** (`mime::draft_from_raw`: headers, text
  part, non-inline attachments; Bcc = envelope recipients not in To/Cc) and open it without
  adding the signature again; the outbox row is removed once the window opens. Closing it
  unsent still asks before discarding.
- **Pre-flight check in the engine too**, before connecting, for queued messages whose
  account limit became known after queuing.

## Risks / Trade-offs

- A server that only advertises `SIZE` after STARTTLS/AUTH: handled for STARTTLS (EHLO is
  repeated after the upgrade); after-AUTH-only servers report no limit and fall back to the
  server's rejection, which is now handled properly.
- Probing opens one extra short SMTP connection per account connection.
