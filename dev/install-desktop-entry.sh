#!/usr/bin/env bash
# Install Tern's .desktop file and app icon for your user (~/.local/share), pointing at a
# Tern you built yourself — what a package does system-wide. The desktop needs them to show
# Tern's name and icon and to bring Tern forward when you click one of its notifications.
#
#   dev/install-desktop-entry.sh [path/to/tern]     # default: target/release/tern
#   dev/install-desktop-entry.sh --remove
set -euo pipefail
cd "$(dirname "$0")/.."
apps=${XDG_DATA_HOME:-$HOME/.local/share}/applications
icons=${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/scalable/apps
if [[ ${1:-} == --remove ]]; then
    rm -f "$apps/dev.tern.Tern.desktop" "$icons/dev.tern.Tern.svg"
else
    bin=$(realpath "${1:-target/release/tern}")
    [[ -x $bin ]] || { echo "no Tern at $bin (cargo build --release -p tern)"; exit 1; }
    mkdir -p "$apps" "$icons"
    sed "s|^Exec=tern |Exec=$bin |" crates/tern/data/dev.tern.Tern.desktop > "$apps/dev.tern.Tern.desktop"
    cp crates/tern/data/dev.tern.Tern.svg "$icons/"
fi
update-desktop-database "$apps" 2>/dev/null || true
gtk-update-icon-cache -q -t "${icons%/scalable/apps}" 2>/dev/null || true
command -v kbuildsycoca6 >/dev/null && kbuildsycoca6 >/dev/null 2>&1 || true
echo "${1:-installed}: $apps/dev.tern.Tern.desktop"
