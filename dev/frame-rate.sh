#!/usr/bin/env bash
# How smoothly something scrolls: record one monitor, then count the frames that actually
# changed in each second. Nobody has to watch the video (it may show private mail).
#
#   dev/frame-rate.sh DP-3 [label]     # records until Enter, then prints the counts
#
# Scroll with steady wheel flicks for ~5 s while it records. A monitor's refresh rate is the
# ceiling (120 at 120 Hz); compare apps or settings on the same monitor. Videos and counts are
# kept in target/frame-rate/. Needs wf-recorder and ffmpeg. Used for md/DEBUG.md 2026-10-27.
set -euo pipefail

output="${1:?usage: $0 OUTPUT [label]   (outputs: hyprctl monitors)}"
label="${2:-$(date +%H%M%S)}"
dir="$(cd "$(dirname "$0")/.." && pwd)/target/frame-rate"
mkdir -p "$dir"
video="$dir/$label.mkv"

# Damage-driven capture, scaled down so the encoder keeps up at 4K/120 Hz.
wf-recorder -y -o "$output" -c libx264 -p preset=ultrafast -p crf=30 -F "scale=1280:-2" \
    -f "$video" >"$dir/$label.log" 2>&1 &
rec=$!
read -rp "Recording $output — scroll now, then press Enter to stop. "
kill -INT "$rec"
wait "$rec" 2>/dev/null || true

# mpdecimate drops frames that barely differ from the one before; what's left changed.
ffmpeg -hide_banner -loglevel info -i "$video" \
    -vf "mpdecimate=hi=64*4:lo=64*2:frac=0.1,showinfo" -f null - 2>&1 |
    grep -oE "pts_time:[0-9.]+" | cut -d: -f2 >"$dir/$label.changed"

python3 -I - "$dir/$label.changed" <<'EOF'
import collections, sys
ts = [float(x) for x in open(sys.argv[1]) if x.strip()]
per = collections.Counter(int(t) for t in ts)
secs = range(int(max(ts)) + 1) if ts else range(0)
print("changed frames per second:", " ".join(str(per[s]) for s in secs))
busy = sorted(per[s] for s in secs if per[s] >= 10)
if busy:
    print(f"while scrolling ({len(busy)} s): average {sum(busy) / len(busy):.0f}, "
          f"median {busy[len(busy) // 2]}, range {busy[0]}–{busy[-1]}")
EOF
