#!/usr/bin/env bash
# A private display for UI tests: a nested Hyprland in a window, so focusing and typing into
# Tern happens on its own seat and never moves focus away from what you're working in.
#
#   dev/test-display.sh start                 # nested Hyprland (state in target/test-display)
#   dev/test-display.sh run [tern args…]      # (re)start `tern --demo [args]` inside it
#   dev/test-display.sh keys TITLE KEY…       # focus window TITLE (Lua pattern) and press keys
#   dev/test-display.sh shot FILE.png         # screenshot the whole nested display
#   dev/test-display.sh stop
#
# KEY is a key name (a, Return, Down, Escape, BackSpace, Tab…), optionally MODS+KEY
# (e.g. CTRL+Return). Text: `keys TITLE m a y a`.
#
# Needs Hyprland and grim. On the outer Hyprland, a window rule for class `aquamarine`
# (the nested window) with `workspace = "8 silent"`, `no_initial_focus = true` and
# `suppress_event = "activate activatefocus"` keeps the window from taking focus.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIR="$ROOT/target/test-display"
mkdir -p "$DIR"

alive() { [[ -f "$DIR/$1.pid" ]] && kill -0 "$(cat "$DIR/$1.pid")" 2>/dev/null; }

# Prints "<instance signature> <wayland socket>" of the nested Hyprland.
inner() {
    alive hyprland || return 1
    local pid
    pid=$(cat "$DIR/hyprland.pid")
    hyprctl instances -j | python3 -c "import json,sys
for i in json.load(sys.stdin):
    if i['pid'] == $pid: print(i['instance'], i['wl_socket']); break
else: sys.exit(1)"
}

# Sets SIG and SOCK, or stops with a hint.
need() {
    local info
    info=$(inner) || { echo "test display not running; use: $0 start" >&2; exit 1; }
    read -r SIG SOCK <<<"$info"
}

case "${1:-}" in
    start)
        if alive hyprland; then echo "already running"; exit 0; fi
        mkdir -p "$DIR/config/hypr"
        # Own XDG_CONFIG_HOME so the nested instance can never pick up your real config
        # (and its autostart apps).
        # Scale 1: the nested output is as many pixels as the outer window is logical pixels,
        # so scale 1 shows apps at the outer desktop's size with the most room (Hyprland would
        # otherwise pick 2 and halve the usable area).
        cat > "$DIR/config/hypr/hyprland.lua" <<'LUA'
-- Test display for Tern: no autostart, no binds; driven by dev/test-display.sh
hl.monitor({ output = "", mode = "preferred", position = "auto", scale = 1 })
LUA
        XDG_CONFIG_HOME="$DIR/config" nohup Hyprland -c "$DIR/config/hypr/hyprland.lua" \
            </dev/null >"$DIR/hyprland.log" 2>&1 &
        echo $! > "$DIR/hyprland.pid"
        for _ in $(seq 50); do inner >/dev/null 2>&1 && break; sleep 0.2; done
        need
        echo "test display on $SOCK (log: $DIR/hyprland.log)"
        ;;
    run)
        shift
        need
        if alive tern; then kill "$(cat "$DIR/tern.pid")"; sleep 1; fi
        (cd "$ROOT" && env -u DISPLAY WAYLAND_DISPLAY="$SOCK" GDK_BACKEND=wayland \
            nohup ./target/debug/tern --demo "$@" </dev/null >"$DIR/tern.log" 2>&1 & echo $! > "$DIR/tern.pid")
        sleep 3
        echo "tern running (log: $DIR/tern.log)"
        ;;
    keys)
        title="$2"; shift 2
        need
        win="window='title:$title'"
        HYPRLAND_INSTANCE_SIGNATURE="$SIG" hyprctl dispatch "hl.dsp.focus({$win})" >/dev/null
        for k in "$@"; do
            mods=""; key="$k"
            if [[ "$k" == *+* ]]; then mods="${k%+*}"; key="${k##*+}"; fi
            HYPRLAND_INSTANCE_SIGNATURE="$SIG" hyprctl dispatch \
                "hl.dsp.send_shortcut({mods='$mods', key='$key', $win})" >/dev/null
            sleep 0.15
        done
        ;;
    shot)
        need
        sleep 0.4
        WAYLAND_DISPLAY="$SOCK" grim "${2:?file}"
        ;;
    stop)
        if alive tern; then kill "$(cat "$DIR/tern.pid")" || true; fi
        if alive hyprland; then kill "$(cat "$DIR/hyprland.pid")"; else echo "not running"; fi
        rm -f "$DIR"/*.pid
        ;;
    *)
        sed -n '2,16p' "$0"
        exit 1
        ;;
esac
