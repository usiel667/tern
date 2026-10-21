# Debug notes

Bugs, investigations and their outcomes. Newest first. For each entry: what you saw,
what caused it, what fixed it (or what's still open).

Handy commands:

```sh
RUST_LOG=tern_sync=debug,tern_imap=debug cargo run   # verbose sync logging
cargo run -- --demo                                  # fake account, no network/keyring
grim -g "X,Y WxH" shot.png                           # screenshot a window (geometry from hyprctl clients -j)
dev/test-imap-server.sh start                        # local Dovecot for live tests (target/dovecot)
eval "$(dev/test-imap-server.sh env)" && cargo test -p tern-imap --test live -- --ignored --nocapture
```

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
focusing it, so focus it first with `hl.dsp.focus({window='title:…'})`. (3) Demo windows go
to workspace 8 via the `tern_demo_ws8` rule in `~/.config/hypr/hyprland.lua`; a rule added
with `hyprctl eval` doesn't take effect, and an exec rule only catches the first window.

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
