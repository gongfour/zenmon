#!/usr/bin/env bash
#
# Synthesise a forklift-fleet key namespace so the TUI can be exercised at a
# scale a flat key list cannot survive.
#
# The tree, the fold threshold and the frame gate only misbehave under many keys
# at a real message rate, and none of that shows up in unit tests. This
# publishes `agv/f000/{pose,battery,state}`-shaped traffic for N vehicles.
#
#   scripts/fleet-load.sh                    # 8 vehicles, 5 Hz, 60s
#   scripts/fleet-load.sh -n 4 -r 1          # something readable by eye
#   scripts/fleet-load.sh -n 200 -e tcp/127.0.0.1:7447   # the fold's case
#   scripts/fleet-load.sh --stop             # kill publishers left behind
#
# Then, in another terminal:  zenmon --mode peer tui
# (--mode is a global flag, so it goes before the subcommand. With --endpoint,
#  point the TUI at the same router instead.)
#
# ## Why N is capped in peer mode
#
# `zenmon pub` publishes one key, so every key is its own process and its own
# zenoh session. Peer mode builds a full mesh between sessions, so the link
# count grows with the square of the process count: measured here, ~60 peers
# already starved the traffic to a couple of messages per second. Past
# PEER_MAX_PROCS this refuses and tells you to use a router, where the
# publishers are clients of one node and the mesh never forms.
#
#   zenohd &                                             # or brew install zenoh
#   scripts/fleet-load.sh -n 200 -e tcp/127.0.0.1:7447
#
set -euo pipefail

VEHICLES=8
RATE=5
DURATION=60s
MODE=peer
ENDPOINT=""
ZENMON="${ZENMON:-./target/release/zenmon}"

# Above this many publisher processes, peer-mode discovery collapses.
PEER_MAX_PROCS=48

usage() {
    sed -n '2,28p' "$0" | sed 's/^# \?//'
    exit "${1:-0}"
}

# Publishers are bounded by --duration, but a run cut short (Ctrl-C, a killed
# parent, a background job whose EXIT trap never fires) leaves them holding
# multicast sockets and skewing the next run's session count.
stop_publishers() {
    if command -v taskkill >/dev/null 2>&1; then
        taskkill //F //IM zenmon.exe >/dev/null 2>&1 || true
    else
        pkill -f "$(basename "$ZENMON") .* pub " >/dev/null 2>&1 || true
    fi
}

while [ $# -gt 0 ]; do
    case "$1" in
        -n|--vehicles) VEHICLES="$2"; shift 2 ;;
        -r|--rate)     RATE="$2";     shift 2 ;;
        -d|--duration) DURATION="$2"; shift 2 ;;
        -e|--endpoint) ENDPOINT="$2"; MODE=client; shift 2 ;;
        --stop)        stop_publishers; echo "publishers stopped"; exit 0 ;;
        -h|--help)     usage 0 ;;
        *) echo "unknown argument: $1" >&2; usage 1 ;;
    esac
done

if [ ! -x "$ZENMON" ]; then
    echo "zenmon not found at $ZENMON — run 'cargo build --release', or set ZENMON=" >&2
    exit 1
fi

procs=$((VEHICLES * 3 + 1))
if [ "$MODE" = peer ] && [ "$procs" -gt "$PEER_MAX_PROCS" ]; then
    cat >&2 <<MSG
$procs publisher processes is too many for peer mode.

Every key is its own process and its own zenoh session, and peer mode meshes
them all together, so past roughly $PEER_MAX_PROCS the traffic starves rather than
scaling. Run a router and point both sides at it:

  zenohd &
  scripts/fleet-load.sh -n $VEHICLES -e tcp/127.0.0.1:7447
  zenmon --endpoint tcp/127.0.0.1:7447 tui

Or lower -n to $((PEER_MAX_PROCS / 3)) or fewer.
MSG
    exit 1
fi

common=(--mode "$MODE")
[ -n "$ENDPOINT" ] && common+=(--endpoint "$ENDPOINT")

trap 'stop_publishers' INT TERM EXIT

echo "publishing $((VEHICLES * 3)) keys at ${RATE}Hz for $DURATION (mode: $MODE)"

for i in $(seq 0 $((VEHICLES - 1))); do
    id=$(printf 'f%03d' "$i")

    # Three topics per vehicle, so the tree has a shape worth collapsing rather
    # than one key per branch.
    "$ZENMON" "${common[@]}" pub "agv/$id/pose" \
        "{\"x\":$i,\"y\":0,\"heading\":90}" \
        --rate "$RATE" --duration "$DURATION" >/dev/null 2>&1 &

    # Battery moves slowly — the field worth plotting once phase D lands.
    "$ZENMON" "${common[@]}" pub "agv/$id/battery" \
        "{\"percent\":$((100 - i % 60)),\"charging\":false}" \
        --rate 1 --duration "$DURATION" >/dev/null 2>&1 &

    # A wide status blob. Vehicle 0 alternates between two states so the diff
    # has something to mark; the rest repeat, which is the case that makes an
    # undiffed pane unreadable.
    if [ "$i" -eq 0 ]; then
        (
            end=$(( $(date +%s) + 600 ))
            while [ "$(date +%s)" -lt "$end" ]; do
                "$ZENMON" "${common[@]}" pub "agv/$id/state" \
                    '{"mode":"moving","speed":1.2,"load":0,"error":null}' >/dev/null 2>&1
                sleep 1
                "$ZENMON" "${common[@]}" pub "agv/$id/state" \
                    '{"mode":"stalled","speed":0.0,"load":0,"error":"obstacle"}' >/dev/null 2>&1
                sleep 1
            done
        ) &
    else
        "$ZENMON" "${common[@]}" pub "agv/$id/state" \
            "{\"mode\":\"idle\",\"error\":null,\"load\":0,\"speed\":0,\"task\":null}" \
            --rate 1 --duration "$DURATION" >/dev/null 2>&1 &
    fi
done

# A second namespace, so the root has more than one child and the tree is not
# trivially a single chain.
"$ZENMON" "${common[@]}" pub "srv/fleet/health" \
    '{"ok":true}' --rate 1 --duration "$DURATION" >/dev/null 2>&1 &

echo "running; Ctrl-C to stop early, or 'scripts/fleet-load.sh --stop' from elsewhere"
wait
