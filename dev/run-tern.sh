#!/usr/bin/env bash
# Run your real Tern and restart it whenever a new release build lands, so you don't have to
# quit and relaunch after each `cargo build --release`.
#
#   dev/run-tern.sh [tern args…]     # Ctrl+C here stops it; closing Tern's window does too
#
# Log (appended): target/tern-real.log. A restart is like quitting: open New Message windows
# close (they autosave to Drafts a few seconds after typing stops), and a message still in its
# Undo Send wait goes out when the new Tern starts.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/target/release/tern"
LOG="$ROOT/target/tern-real.log"
[[ -x "$BIN" ]] || { echo "no release build yet: cargo build --release -p tern" >&2; exit 1; }

mtime() { stat -c %Y "$BIN" 2>/dev/null || echo 0; }
note() { echo "[run-tern $(date +%T)] $*" | tee -a "$LOG"; }

pid=""
stop() {
    [[ -n "$pid" ]] && kill "$pid" 2>/dev/null && wait "$pid" 2>/dev/null
    pid=""
}
trap 'stop; exit 0' INT TERM

while true; do
    seen=$(mtime)
    "$BIN" "$@" >>"$LOG" 2>&1 &
    pid=$!
    note "started Tern (pid $pid)"
    restart=""
    while kill -0 "$pid" 2>/dev/null; do
        sleep 2
        now=$(mtime)
        [[ "$now" == "$seen" ]] && continue
        # Wait until the build has finished writing the binary.
        sleep 3
        [[ "$(mtime)" == "$now" && -x "$BIN" ]] || continue
        note "new build: restarting"
        command -v notify-send >/dev/null && notify-send -a Tern "Tern" "Restarting with the new build"
        restart=1
        stop
        break
    done
    if [[ -z "$restart" ]]; then
        wait "$pid" 2>/dev/null
        note "Tern closed; not restarting"
        exit 0
    fi
done
