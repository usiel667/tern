# Debug notes

Bugs, investigations and their outcomes. Newest first. For each entry: what you saw,
what caused it, what fixed it (or what's still open).

Handy commands:

```sh
RUST_LOG=tern_sync=debug,tern_imap=debug cargo run   # verbose sync logging
cargo run -- --demo                                  # fake account, no network/keyring
grim -g "X,Y WxH" shot.png                           # screenshot a window (geometry from hyprctl clients -j)
```

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
