#!/usr/bin/env bash
# A private display for UI tests: a nested Hyprland in a window, so focusing and typing into
# Tern happens on its own seat and never moves focus away from what you're working in.
#
#   dev/test-display.sh start                 # nested Hyprland (state in target/test-display)
#   dev/test-display.sh run [--local-dialogs] [tern args…]
#                                             # (re)start `tern --demo [args]` inside it;
#                                             # --local-dialogs keeps file dialogs in the test
#                                             # display (no portal), at the cost of desktop theming
#   dev/test-display.sh keys TITLE KEY…       # focus window TITLE (Lua pattern) and press keys
#   dev/test-display.sh type TITLE TEXT       # type TEXT (letters, digits, common punctuation)
#   dev/test-display.sh pointer CMD…          # virtual mouse: move X Y | glide X Y | click |
#                                             #   down | up | sleep MS | drag X1 Y1 X2 Y2
#   dev/test-display.sh shot FILE.png         # screenshot the whole nested display
#   dev/test-display.sh stop
#
# KEY is a key name (a, Return, Down, Escape, BackSpace, Tab…), optionally MODS+KEY
# (e.g. CTRL+Return). Text: `keys TITLE m a y a`.
#
# Needs Hyprland and grim; `pointer` also needs gcc, wayland-scanner and wayland-client
# (builds dev/vpointer on first use). On the outer Hyprland, a window rule for class `aquamarine`
# (the nested window) with `workspace = "8 silent"`, `no_initial_focus = true` and
# `suppress_event = "activate activatefocus"` keeps the window from taking focus, and
# `render_unfocused = true` keeps it drawing while workspace 8 isn't on screen.
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

# Stop the demo this script started and wait until it's gone, so a new one doesn't hand
# its arguments to the old instance (GApplication is single-instance) and exit.
stop_tern() {
    alive tern || return 0
    local pid
    pid=$(cat "$DIR/tern.pid")
    kill "$pid" 2>/dev/null || true
    for _ in $(seq 25); do kill -0 "$pid" 2>/dev/null || return 0; sleep 0.2; done
    kill -9 "$pid" 2>/dev/null || true
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
        portals=""
        if [[ "${1:-}" == --local-dialogs ]]; then portals="no-portals"; shift; fi
        need
        stop_tern
        # A single background command, so $! is tern itself (env and nohup exec into it).
        cd "$ROOT"
        env -u DISPLAY WAYLAND_DISPLAY="$SOCK" GDK_BACKEND=wayland GDK_DEBUG="$portals" \
            nohup ./target/debug/tern --demo "$@" </dev/null >"$DIR/tern.log" 2>&1 &
        echo $! > "$DIR/tern.pid"
        sleep 3
        if ! alive tern; then
            # GApplication handed the arguments to a demo already running elsewhere and exited.
            echo "tern exited at once: another 'tern --demo' is running (pgrep -a -x tern)" >&2
            exit 1
        fi
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
    type)
        title="$2"; text="$3"
        keys=()
        for ((i = 0; i < ${#text}; i++)); do
            ch="${text:i:1}"
            case "$ch" in
                [a-z0-9]) keys+=("$ch") ;;
                [A-Z]) keys+=("SHIFT+${ch,,}") ;;
                " ") keys+=(space) ;;
                /) keys+=(slash) ;;
                .) keys+=(period) ;;
                ,) keys+=(comma) ;;
                -) keys+=(minus) ;;
                _) keys+=(SHIFT+minus) ;;
                "~") keys+=(SHIFT+grave) ;;
                @) keys+=(SHIFT+2) ;;
                :) keys+=(SHIFT+semicolon) ;;
                *) echo "can't type '$ch'" >&2; exit 1 ;;
            esac
        done
        exec "$0" keys "$title" "${keys[@]}"
        ;;
    pointer)
        shift
        need
        bin="$DIR/vpointer"
        src="$ROOT/dev/vpointer"
        if [[ ! -x "$bin" || "$src/vpointer.c" -nt "$bin" ]]; then
            wayland-scanner client-header "$src/wlr-virtual-pointer-unstable-v1.xml" \
                "$DIR/wlr-virtual-pointer-unstable-v1-client-protocol.h"
            wayland-scanner private-code "$src/wlr-virtual-pointer-unstable-v1.xml" "$DIR/wlr-virtual-pointer-protocol.c"
            gcc -O2 -Wall -I"$DIR" -o "$bin" "$src/vpointer.c" "$DIR/wlr-virtual-pointer-protocol.c" -lwayland-client
        fi
        read -r W H < <(HYPRLAND_INSTANCE_SIGNATURE="$SIG" hyprctl monitors -j |
            python3 -c "import json,sys;m=json.load(sys.stdin)[0];print(round(m['width']/m['scale']), round(m['height']/m['scale']))")
        WAYLAND_DISPLAY="$SOCK" "$bin" "$W" "$H" "$@"
        ;;
    shot)
        need
        sleep 0.4
        # The nested display only draws while its window is visible (or has the
        # render_unfocused rule); don't hang forever if it isn't.
        WAYLAND_DISPLAY="$SOCK" timeout 10 grim "${2:?file}" ||
            { echo "screenshot timed out: is workspace 8 visible / render_unfocused set?" >&2; exit 1; }
        ;;
    stop)
        stop_tern
        if alive hyprland; then kill "$(cat "$DIR/hyprland.pid")"; else echo "not running"; fi
        rm -f "$DIR"/*.pid
        ;;
    *)
        sed -n '2,16p' "$0"
        exit 1
        ;;
esac
