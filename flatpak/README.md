# Flatpak

`dev.tern.Tern.json` builds Tern on the GNOME 51 runtime (it has WebKitGTK 6 and
libadwaita) with the Rust SDK extension, entirely offline: `cargo-sources.json` lists every
crate in `Cargo.lock` with its checksum.

Permissions: network, Wayland (X11 as a fallback), GPU, and `org.freedesktop.secrets` — the
desktop's keyring (gnome-keyring/oo7 on GNOME, ksecretd/KWallet on Plasma), where passwords
are kept. Opening and saving attachments, notifications and the dark-mode setting go
through portals, which need no permission.

```sh
flatpak install --user flathub org.gnome.Sdk//51 org.gnome.Platform//51 \
    org.freedesktop.Sdk.Extension.rust-stable//26.08
cd flatpak
flatpak-builder --user --force-clean --repo=repo build dev.tern.Tern.json
flatpak build-bundle repo tern.flatpak dev.tern.Tern     # one file to hand to testers
flatpak install --user tern.flatpak                      # (needs org.gnome.Platform//51)
```

When `Cargo.lock` changes, regenerate the sources with
[flatpak-cargo-generator](https://github.com/flatpak/flatpak-builder-tools/tree/master/cargo):

```sh
python3 flatpak-cargo-generator.py ../Cargo.lock -o cargo-sources.json
```
