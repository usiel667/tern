# Debug notes

Bugs, investigations and their outcomes. Newest first. For each entry: what you saw,
what caused it, what fixed it (or what's still open).

Handy commands:

```sh
RUST_LOG=tern_sync=debug,tern_imap=debug cargo run   # verbose sync logging
cargo run -- --demo                                  # fake account, no network/keyring
dev/run-tern.sh                                      # your real Tern; restarts itself after each cargo build --release -p tern
grim -g "X,Y WxH" shot.png                           # screenshot a window (geometry from hyprctl clients -j)
dev/test-display.sh start                            # private nested Hyprland for UI tests (never steals focus)
dev/test-display.sh run 'mailto:?subject=x'          # tern --demo inside it (--local-dialogs: file dialogs stay inside)
dev/test-display.sh type 'x' 'hello'                 # type text into window titled x
dev/test-display.sh keys 'x' c h Down Return         # focus the window whose title contains regex x, press keys
dev/test-display.sh pointer drag 100 800 300 400      # virtual mouse (move/glide/click/down/up/drag)
dev/test-display.sh shot shot.png                    # screenshot the test display
dev/test-imap-server.sh start                        # local Dovecot for live tests (target/dovecot)
eval "$(dev/test-imap-server.sh env)" && cargo test -p tern-imap --test live -- --ignored --nocapture
dev/live-test.sh .env.gmail-test                     # same tests on the throwaway Gmail (git-ignored settings; wipes its INBOX/Trash)
```

## 2026-10-27 — Deleting a message scrolled the list to the top

**Seen (demo, nested test display):** select a message 15 rows down, press Delete: the next
message is selected but the list shows the top. Restoring the scroll value in an idle
(tried 10-25) didn't help.

**Cause:** `select_after_removal` moved the *selection* to the next row but left keyboard
*focus* on the row being deleted. GTK's ListView anchors its scroll position to the focused
row; when that row is removed, the anchor falls back to the top.

**Fixed:** it also moves focus there (`scroll_to(…, FOCUS)`, as j/k already do). Same test:
the list stays where it was and the next message takes the deleted one's place.

## 2026-10-27 — A sync round with nothing new took 9–17 s

**Seen:** each mailbox cost STATUS, SELECT, UID FETCH of new mail, FETCH CHANGEDSINCE
and UID SEARCH, one after another on one connection, even when nothing had changed. On
the test account: 9 mailboxes, 300–590 ms each, 3.7 s a round. Gmail has CONDSTORE but
not QRESYNC, so the existing "nothing changed" shortcut (QRESYNC only) never applied.

**Found on Gmail (probed on the test account):**
- HIGHESTMODSEQ is account-wide: appending to INBOX raised it in every mailbox. It can't
  tell you one mailbox is unchanged, only that the whole account is.
- STATUS lags about 10 s behind changes made on *other* connections (measured 9.9 s and
  10.3 s), whether or not the mailbox is selected. Changes made on the same connection
  show at once. My first version trusted STATUS and the live test caught a missed flag
  change.

**Fixed:** the cursor (`ImapState` v3) also records UIDNEXT and the message count.
- If STATUS shows the same UIDVALIDITY, HIGHESTMODSEQ, UIDNEXT and count, the sync stops
  there (`unchanged_since`): 1 command instead of 5. The cursor doesn't move, so a change
  STATUS hasn't caught up with arrives in a later sync (`stale_status_only_delays_a_change`).
  A mailbox IDLE reported (`pushed`) skips this check, so pushed new mail isn't delayed.
- Otherwise, the same UIDNEXT skips the new-mail fetch, and the same count as well skips
  the deletion search (CONDSTORE mode): 3 commands instead of 5.
- A quiet round on the test account: 3.7 s → 0.9 s (~100 ms a mailbox).
- Your real Gmail (18 mailboxes): 9–17 s → 2.7–3.8 s a round (the log line includes
  bodies downloaded ahead alongside it).

- A push that arrived while its mailbox was already syncing used to be dropped (the sync in
  flight may have looked before the change); it now queues one more sync after it
  (`push_during_a_sync_syncs_again_after_it`).
- The log says how long each round took ("sync round finished").

Not changed: the unread counts come from STATUS too, so they can be ~10 s behind changes
from other devices (as before).

## 2026-10-26 — Inbox messages took ~2 s to open after bodies were downloaded ahead

**Seen:** after the download-ahead (below) filled in every Inbox body, opening an Inbox message
took 1.3–2.6 s, all of it logged as `rendering`; other folders and the demo stayed fast. The same
HTML in a bare WebKitGTK view (test display and real display) loaded in 3–60 ms, so WebKit
wasn't it: Tern's main thread was busy while the page loaded.

**Cause:** opening an unread message marks it read, which reloads the list, and `Store::list`
for the Inbox took 1.4 s (0.36 s without conversations). `one_copy` selected `m.*` into its
`ROW_NUMBER()` window, so every cached `body_html`/`raw` went through the sort. Before the
download-ahead almost no Inbox rows had a body, which hid it.

**Fixed:** `one_copy` selects `SUMMARY_COLS` only (all four callers read just those): 14 ms with
conversations, 5 ms without, on a copy of the same database. Lesson: anything that puts big
columns on hot rows needs the list queries re-timed.

**Then still ~0.5 s, and the list jumped to the top on every open:** a new `slow ui step` log
(main-thread steps ≥ 30 ms) showed each list reload took ~400 ms with a 15 ms query: `reload`
swapped all 500 rows into the model, so GTK rebuilt the list, and replacing the focused row
resets the ListView's scroll. Reload now updates rows in place when the same messages are in
the same order (replace the boxed value, redraw the visible row widget), and otherwise splices
only the changed stretch; restoring the selection no longer re-renders the viewer. Opens are
now under 0.3 s and reloads under 30 ms. Deleting a message also scrolled the list to the
top (fixed 2026-10-27, below).

## 2026-10-25 — Opening a message still took ~0.5 s (and some downloaded twice)

**Seen:** with the slow-open log (`slow message open` / `slow body fetch`, kept in), typical
messages (12–150 KB) took 0.4–0.6 s, about 90% of it waiting for Gmail and 2–100 ms rendering;
a 1.8 MB one 1.2 s and a 17 MB one 2.1 s. Some messages were fetched twice: the viewer asks
again while the first download is still running.

**Fixed:** bodies are downloaded ahead in the background (`Engine::prefetch`): newest first
from the inboxes and every mailbox the user opens, one at a time per account, on its own IMAP
connection (`prefetch_body`), not counted as busy, skipping messages over 5 MB
(`PREFETCH_MAX_SIZE`). A copy of a message with the same Message-ID shares its body
(`Store::body`), so Gmail's All Mail copy needn't be downloaded again. A body already on its
way isn't fetched a second time (`bodies_fetching`). Still open: big messages download whole,
attachments and all, when opened.

## 2026-10-23 — Opening a message could wait behind a whole sync round

**Seen:** on the real Gmail account, a sync round with nothing new took 9–17 s (18
mailboxes, 0.3–2 s each: STATUS, SELECT, FETCH CHANGEDSINCE, UID SEARCH). The engine spawns
every mailbox's sync at once, and they all share the one command connection, whose tokio
`Mutex` is first-come first-served, so a click during a round queued behind the rest of it.
The log also showed Gmail dropping the idle command connection ("Connection reset by peer"),
which failed the next command instead of reconnecting.

**Wrong guess first:** I blamed SELECT (entry below). Measured on the real account, SELECT /
EXAMINE takes ~120 ms, even on All Mail with 23k messages.

**Fixed:** `fetch_body` has its own connection (`ImapBackend::body`), so opening a message
mid-round took 124 ms. A connection unused for 60 s gets a NOOP first and is replaced if
that fails (no blind retry: an APPEND might have gone through). Each connection remembers
its selected mailbox and skips SELECT for UID commands (`Conn::open`); sync, older-mail
loads and the post-APPEND search still SELECT afresh (`Conn::reselect`) for current
UIDVALIDITY/HIGHESTMODSEQ. Not covered by a test: the stale-connection NOOP.

## 2026-10-22 — List previews showed base64 / quoted-printable

**Saw (real Gmail, screenshot):** previews like `R2V0IHN0YXJ0ZWQgd2l0aCBH…` (base64 of "Get started
with Google…") and `=E2=80=8C =E2=80=8C…` (quoted-printable zero-width padding).

**Cause:** the list fetch takes the first 2 KB of the body (`BODY.PEEK[TEXT]<0.2048>`). In
multipart mail that cut ends inside a part, the multipart never closes, and mail-parser leaves
the unterminated part undecoded. (A single-part cut decodes fine.) A cut inside a QP escape
(`=E`) also ate the characters before it.

**Fix:** `close_open_multiparts` ends every multipart the prefix opened; a dangling `=`/`=X` is
trimmed; zero-width padding characters are dropped from previews. Checked on 2,074 of the
user's real messages (export): quoted-printable-looking previews 75 → 0; three replies whose
first 2 KB is only quoted text now have no preview (correct: quotes are skipped; before, raw
`=3E` hid the `>`). Schema v7 makes every mailbox fetch its list again once; a full resync
without renumbering now updates rows in place, keeping downloaded bodies (and not
double-counting contacts).

## 2026-10-22 — Gmail account showed three Sent folders

**Saw:** the sidebar listed Sent three times (and Trash/Drafts more than once) for the
user's Gmail: `[Gmail]/Sent Mail` plus old labels `Sent Messages` (132 messages, 2011–2013)
and `Sent` (3), and labels `Trash` (7) and `Deleted Messages` (0) — the names an iPhone or
Apple Mail creates when an account is set up over IMAP.

**Cause:** `role_for` trusts SPECIAL-USE attributes but also guesses roles from names for
servers without them; on Gmail both applied, so every match became Sent/Trash.
`mailbox_by_role` then took whichever came first — sent copies or deletes could have gone to
an old label.

**Fix:** `one_per_role` keeps one mailbox per role (the server-marked one, else the first name
match); the rest become ordinary folders with their mail intact. Before changing anything the
user asked for a copy of those mailboxes: `examples/export_mailboxes.rs` (read-only EXAMINE +
BODY.PEEK, `.eml` files + index.csv).

## 2026-10-22 — Gmail: every conversation counted one message too many

**Saw (first real Gmail account):** a single "Welcome to Google…" message showed **2** next
to its time, a two-message thread **3**; the Unread badge was about twice the real number.

**Cause:** Gmail exposes one message in several IMAP mailboxes (Inbox, All Mail, often
Important and Starred), so the cache holds a copy per mailbox. The conversation size was a
plain `COUNT(*)` over the thread across mailboxes, the conversation view listed each copy,
Flagged/Unread/search listed one row per copy, and the Unread badge summed every
mailbox's unread count. On the user's cache: 1,827 unread copies for 934 unread messages;
995 Inbox conversations miscounted.

**Fix:** `one_copy()` in tern-store keeps one row per Message-ID per account wherever a list
spans mailboxes (list, thread, search, new `Store::count`), preferring a real mailbox over
All Mail / Important / Starred; conversation size counts distinct Message-IDs. Messages
without a Message-ID always count. Test: `gmail_copies_count_once`.

**Then:** unread counts came from the cache, which holds only the newest 1000 messages per
mailbox at first (934 vs Gmail's 4,832). Fixed by server counts (OpenSpec
`server-mailbox-counts`): IMAP `STATUS (MESSAGES UNSEEN)` before each sync's SELECT, JMAP
`Mailbox/get`; shown = server + (cache now − cache at the snapshot), older batches move the
snapshot.

**Also measured:** the long-session live test took 34 min on Gmail (~2.8 s per sync +
body fetch pair). I blamed Gmail's SELECT; wrong — see the 2026-10-23 entry above.

## 2026-10-22 — Live tests against real Gmail

**Saw:** both IMAP live tests failed on a throwaway Gmail. **Causes (test assumptions, not
Tern bugs):** Gmail has no Archive mailbox (All Mail; the Archive button already falls back
to it); EXPUNGE in All Mail deletes nothing (Gmail deletes via Trash; Tern's Delete moves
there); Gmail offers CONDSTORE but not QRESYNC; the IDLE watcher signed in slower than the
1.5 s pause, so the APPEND came before IDLE started. **Fix:** a scratch label
(`create_mailbox`), the test reads which extensions are in use (`extensions()`), the
watcher signs in first. **Measured:** Gmail's IDLE push for an APPENDed message took 35 s
and over 45 s (Dovecot: immediate); real incoming mail still to measure.

## 2026-10-21 — Test display keys went nowhere

**Saw (testing Undo Send):** `dev/test-display.sh keys 'Tern' CTRL+n` did nothing; nested
`hyprctl dispatch` printed "hl.focus: window not found" (the script hid its output).

**Cause / fix:** Hyprland's `title:` matches the *whole* title (now "Tern — 3 unread"), and a
compose window's title follows its subject while typing, so even an exact title went stale
mid-`type`. `keys` now finds the first window whose title *contains* the regex, then
addresses it by `address:` for every key.

## 2026-10-21 — Refused JMAP sends vanished; Ctrl+Return in the body didn't send

**Saw (code review while adding size limits):** `JmapBackend::submit` ignored `notCreated`
in the `Email/import` / `EmailSubmission/set` responses, so a submission the server refused
(e.g. `tooLarge`) returned Ok — the engine deleted it from the outbox as sent. Upload 413s
were `Network` errors and retried forever. Separately, in the test display Ctrl+Return with
the cursor in the message body inserted a line break instead of sending.

**Cause / fix:** JMAP now turns `notCreated` and 413 into `Error::Rejected` (kept as
unsent, not retried). The body `TextView` handles Ctrl+Return before the window's
bubble-phase key controller; sending moved to the capture-phase controller (next to
Shift+Ctrl+A). Earlier send tests had focus in the To field, which is why it went unseen.

**Also:** smtp.gmail.com advertises `SIZE 35882577`, smtp.mail.me.com `SIZE 28319744`
(2026-10-21, `cargo test -p tern-smtp --test live -- --ignored`).

## 2026-10-20 — Dropping files from Dolphin did nothing

**Saw:** dragging a file from Dolphin onto a New Message attached nothing (owner's report).
Reproduced in the test display with Dolphin + `dev/test-display.sh pointer drag …`;
`WAYLAND_DEBUG` showed Tern answering every motion with `set_actions(0, 0)` / `accept(nil)`.
A bare 5-line PyGObject window with `DropTarget(FileList, COPY)` failed the same way.

**Cause:** Dolphin offers copy|move and Hyprland selects **move**; GTK's `DropTarget`
rejects a drop whose selected action isn't in its own actions, and ours was COPY only.
Allowing COPY|MOVE makes it accept, but GTK then finishes with the compositor's choice
(`set_actions(2, 2)`, a move) even if `enter`/`motion` return COPY — and a source may
delete the original after a finished move (Dolphin didn't, but that's not ours to bet on).

**Fix:** `DropTargetAsync` with COPY|MOVE, reading the files ourselves and calling
`drop.finish(COPY)` (log shows `set_actions(1, 1)` before `finish()`). Also: GTK prefers
the document portal's `application/vnd.portal.filetransfer`; when that read fails (no
document portal, or source on another D-Bus session) we fall back to `text/uri-list`.

**Lesson:** test drops with a real file manager as the source; GTK-to-GTK drags negotiate
copy and would have hidden this.

## 2026-10-20 — Autocomplete popover mapped but never drawn

**Saw:** the recipient-suggestion `gtk::Popover` (parented to the To `GtkEntry`) reported
`is_visible() && is_mapped()` after `popup()` but had a 0×0 size and nothing appeared.

**Cause:** GTK4 only sizes and positions a popover when its parent calls
`gtk_popover_present()` in `size_allocate`. `GtkEntry` does that for its own popovers
(emoji chooser, context menu) but not for one you `set_parent()` on it.

**Fix:** call `popover.present()` right after `popup()` (`autocomplete.rs`).

**Also learned while testing:** (1) GTK fires `changed` before moving the cursor, so a
"cursor at end" check inside `changed` always fails. (2) Under Hyprland, popovers only show
while their window is focused; `hl.dsp.send_shortcut` types into a window without
focusing it, so focus it first with `hl.dsp.focus({window='title:…'})` — but doing that on
the real desktop steals the developer's focus mid-typing. UI tests now run in a nested
Hyprland (`dev/test-display.sh`), which has its own seat, so focus and keys stay inside it.
(3) Window rules: a rule added with `hyprctl eval` doesn't take effect (put it in
`hyprland.lua`), and an exec rule only catches a process's first window.

---

## 2026-10-17 — Unsolicited-response buffer: suspected overflow, not reproduced

**Suspected (while designing CONDSTORE/QRESYNC):** async-imap delivers `VANISHED` (and other
unsolicited notices) through a channel bounded at 100 that drops new items when full, and Tern
never read it, so on a long-lived connection deletion notices could be lost.

**Measured:** probed the channel during the live `change_tracking` test against Dovecot 2.4
after 240 commands across two mailboxes: it held at most one item (the `ENABLED` reply or a
`VANISHED`). Re-selecting the mailbox for every command, as Tern does, keeps it nearly empty,
and `[CLOSED]` doesn't reach it. The live test passes with or without draining.

**Outcome:** `drain_vanished` empties it before each QRESYNC fetch anyway (one cheap call) in
case other servers send more notices. Not a confirmed bug.

---

## 2026-10-16 — IMAP reported a dropped connection as a protocol error

**Seen (while designing the offline queue):** every IMAP command error went through
`map_err(proto)`, so a connection lost mid-command (`async_imap::Error::Io` /
`ConnectionLost`) became `Error::Protocol`, the same as a server `NO`/`BAD`. The engine
couldn't tell "offline, retry later" from "the server refused", so offline actions would
have been rolled back instead of queued.

**Fix:** `connect::cmd` classifies command errors: `Io` and `ConnectionLost` →
`Error::Network`, everything else → `Error::Protocol`. All command call sites in
`tern-imap/src/lib.rs` use it. Test: `dead_connection_is_network_server_refusal_is_protocol`.

**Status:** fixed.

---

## 2026-10-16 — Wrong password made the engine reconnect in a tight loop

**Seen (code review, milestone 2):** any `Error::Auth` rebuilt the backend immediately,
which failed again, forever, with a toast each time. Several tasks failing at once each
started their own rebuild. If the rebuild itself failed (e.g. Google token refresh while
offline) the account stayed dead until restart. A rebuild finishing after the account was
removed would re-add it.

**Fix:** per-account `AuthRetry` in the engine: first reconnect immediate, then
`auth_retry_delay` (30 s doubling, capped at 30 min); only one reconnect pending per
account and only that failure toasts; `Internal::ReconnectFailed` reschedules; a
successful mailbox list resets the streak; `AccountReady { reconnect: true }` is dropped
for removed accounts. The backend builder is now `Engine::connect` so tests can inject the
fake. Tests run an hour on Tokio's paused clock: `wrong_password_backs_off_instead_of_looping`,
`removed_account_is_not_resurrected_by_a_pending_reconnect`.

**Status:** fixed.

---

## 2026-10-16 — Outbox could send the same email twice, and lost Bcc on retry

**Seen (code review, milestone 2):** `flush_outbox` (startup + every 5 min) re-dispatched
every outbox row, including ones whose send was still in flight, so a slow send could go
out twice. Retried sends rebuilt recipients from the headers, which never contain Bcc.
Failed sends weren't recorded (`outbox_failed` was never called).

**Fix:** the engine tracks in-flight outbox ids (`Engine::sending`) and skips them; the
SMTP envelope is stored with each row (`outbox.envelope`, schema v2 + migration); a new
`Internal::SendFailed` records attempts and last error. Tests:
`flush_during_send_does_not_send_twice` (fails with 2 sends without the guard),
`offline_send_keeps_bcc_when_retried`, `migrates_v1_outbox`.

**Status:** fixed.

---

## 2026-10-15 — Window see-through but not blurred on Hyprland

**Seen:** Tern's window showed a sharp, unblurred wallpaper behind it, making text hard
to read. Other apps (Warp, kitty) were blurred normally.

**Cause:** not Tern. Hyprland 0.56 skips blur for apps built on GTK 4.24.x / libadwaita
1.10.x ([hyprwm/Hyprland#16508](https://github.com/hyprwm/Hyprland/issues/16508)).
pavucontrol (also GTK4) has the same problem. The global `active_opacity` 0.90 /
`inactive_opacity` 0.75 from HyDE still applies, so the window is translucent with no blur.

**Ruled out:** making the window background translucent in CSS (to clear GTK's opaque
region), `GDK_DISABLE=offload`, `GSK_RENDERER=gl` / `cairo`. None restored blur.

**Fix (workaround):** `tern_opaque` window rule in `~/.config/hypr/hyprland.lua` sets
Tern to `opacity 1.0 override 1.0 override`. Remove it once #16508 is fixed. The other
option is to downgrade gtk4 to 4.22.5 and libadwaita to 1.9.4.

**Status:** open upstream.
