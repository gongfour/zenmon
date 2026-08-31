#!/usr/bin/env bash
#
# Synthesise a forklift-fleet key namespace so the TUI can be exercised at a
# scale a flat key list cannot survive.
#
# The tree, the fold threshold and the frame gate all only misbehave under many
# keys at a real message rate, and none of that shows up in unit tests. This
# publishes `agv/f000/{pose,battery,state}`-shaped traffic for N vehicles.
#
# Peer mode by default, so no zenohd is needed: the publishers and the TUI find
# each other over multicast scouting. Pass --endpoint to go through a router.
#
#   scripts/fleet-load.sh                      # 50 vehicles, 5 Hz, 60s
#   scripts/fleet-load.sh -n 200 -r 10 -d 5m   # the case the fold exists for
#   scripts/fleet-load.sh -n 3 -r 1            # something readable by eye
#
# Then, in another terminal:  zenmon --mode peer tui
# (--mode is a global flag, so it goes before the subcommand.)
#
set -euo pipefail

VEHICLES=50
RATE=5
DURATION=60s
MODE=peer
ENDPOINT=""
ZENMON="${ZENMON:-./target/release/zenmon}"

usage() {
    sed -n '2,17p' "$0" | sed 's/^# \?//'
    exit "${1:-0}"
}

while [ $# -gt 0 ]; do
    case "$1" in
        -n|--vehicles) VEHICLES="$2"; shift 2 ;;
        -r|--rate)     RATE="$2";     shift 2 ;;
        -d|--duration) DURATION="$2"; shift 2 ;;
        -e|--endpoint) ENDPOINT="$2"; MODE=client; shift 2 ;;
        -h|--help)     usage 0 ;;
        *) echo "unknown argument: $1" >&2; usage 1 ;;
    esac
done

if [ ! -x "$ZENMON" ]; then
    echo "zenmon not found at $ZENMON — run 'cargo build --release', or set ZENMON=" >&2
    exit 1
fi

common=(--mode "$MODE")
[ -n "$ENDPOINT" ] && common+=(--endpoint "$ENDPOINT")

pids=()
cleanup() {
    # The publishers are bounded by --duration, but Ctrl-C should not leave
    # dozens of sessions holding multicast sockets open.
    trap - INT TERM EXIT
    [ ${#pids[@]} -gt 0 ] && kill "${pids[@]}" 2>/dev/null || true
    wait 2>/dev/null || true
}
trap cleanup INT TERM EXIT

echo "publishing $((VEHICLES * 3)) keys at ${RATE}Hz for $DURATION (mode: $MODE)"

for i in $(seq 0 $((VEHICLES - 1))); do
    id=$(printf 'f%03d' "$i")

    # Three topics per vehicle, so the tree has a shape worth collapsing rather
    # than one key per branch.
    "$ZENMON" "${common[@]}" pub "agv/$id/pose" \
        "{\"x\":$i,\"y\":0,\"heading\":90}" \
        --rate "$RATE" --duration "$DURATION" >/dev/null 2>&1 &
    pids+=($!)

    # Battery moves slowly and monotonically — the field worth plotting once
    # phase D lands.
    "$ZENMON" "${common[@]}" pub "agv/$id/battery" \
        "{\"percent\":$((100 - i % 60)),\"charging\":false}" \
        --rate 1 --duration "$DURATION" >/dev/null 2>&1 &
    pids+=($!)

    # A wide status blob that mostly repeats: this is what the phase C diff has
    # to make readable.
    "$ZENMON" "${common[@]}" pub "agv/$id/state" \
        "{\"mode\":\"idle\",\"error\":null,\"load\":0,\"speed\":0,\"task\":null}" \
        --rate 1 --duration "$DURATION" >/dev/null 2>&1 &
    pids+=($!)
done

# A second namespace, so the root has more than one child and the tree is not
# trivially a single chain.
"$ZENMON" "${common[@]}" pub "srv/fleet/health" \
    '{"ok":true}' --rate 1 --duration "$DURATION" >/dev/null 2>&1 &
pids+=($!)

echo "${#pids[@]} publishers running; Ctrl-C to stop early"
wait
