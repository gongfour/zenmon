#!/usr/bin/env bash
# driver.sh — bounded automation for the zenmon Zenoh monitor CLI.
#
# Run from anywhere (Bash / Git Bash on Windows):
#   ./driver.sh smoke
#   ./driver.sh snapshot [zenmon global flags...]
#   ./driver.sh capture <keyexpr> <duration> [zenmon global flags...]
#
# Examples:
#   ./driver.sh snapshot -e tcp/127.0.0.1:7447
#   ./driver.sh capture 'demo/**' 5s -e tcp/127.0.0.1:7447
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"

find_bin() {
  local cand
  for cand in "$ROOT/target/release/zenmon.exe" "$ROOT/target/release/zenmon"; do
    if [ -f "$cand" ]; then BIN="$cand"; return 0; fi
  done
  echo "zenmon binary not found — building release binary..." >&2
  (cd "$ROOT" && cargo build --release) || return 1
  for cand in "$ROOT/target/release/zenmon.exe" "$ROOT/target/release/zenmon"; do
    if [ -f "$cand" ]; then BIN="$cand"; return 0; fi
  done
  return 1
}

find_bin || { echo "FATAL: could not build zenmon" >&2; exit 1; }

CMD="${1:-}"
shift 2>/dev/null || true
SMOKE_TMP=""

run_smoke() {
  local config_path pass=0 fail=0 zpid="" subpid="" info
  # Keep temporary config under the repository so native Windows executables
  # receive a path they can resolve when the driver runs through Git Bash.
  SMOKE_TMP="$(mktemp -d "$ROOT/.zenmon-smoke.XXXXXX")"

  cleanup() {
    [ -n "${subpid:-}" ] && kill "$subpid" 2>/dev/null
    [ -n "${zpid:-}" ] && kill "$zpid" 2>/dev/null
    rm -rf "${SMOKE_TMP:-}"
  }
  trap cleanup EXIT

  ok()  { echo "PASS  $1"; pass=$((pass+1)); }
  bad() { echo "FAIL  $1"; fail=$((fail+1)); }

  echo "== Tier A: router-less peer-to-peer =="
  printf '%s\n' '{
    listen: { endpoints: ["tcp/127.0.0.1:17447"] },
    connect: { timeout_ms: 1000, exit_on_failure: false }
  }' > "$SMOKE_TMP/listen.json5"
  config_path="$SMOKE_TMP/listen.json5"
  if command -v cygpath >/dev/null 2>&1; then
    config_path="$(cygpath -w "$config_path")"
  elif command -v wslpath >/dev/null 2>&1; then
    config_path="$(wslpath -w "$config_path")"
  fi

  "$BIN" -m peer -c "$config_path" -e tcp/127.0.0.1:9 --scout-port 7548 \
      --json sub 'zenmon-smoke/**' --duration 6s > "$SMOKE_TMP/a.jsonl" 2>"$SMOKE_TMP/a.log" &
  subpid=$!
  sleep 2

  if ! kill -0 "$subpid" 2>/dev/null; then
    bad "hub subscriber failed to start"
    sed 's/^/      /' "$SMOKE_TMP/a.log"
    subpid=""
    echo
    echo "smoke: $pass passed, $fail failed"
    return 1
  fi

  if "$BIN" -m peer -e tcp/127.0.0.1:17447 --scout-port 7549 \
      pub zenmon-smoke/p2p '{"n":1}' --att '{"source":"smoke"}' 2>/dev/null; then
    ok "p2p publish returned 0"
  else
    bad "p2p publish failed"
  fi

  info="$("$BIN" -m peer -e tcp/127.0.0.1:17447 --scout-port 7549 --json info 2>/dev/null || true)"
  if [ -z "$info" ] || printf '%s' "$info" | grep -q '"peers":\[\]'; then
    bad "connector sees no hub peer"
  else
    ok "connector sees hub peer"
  fi

  wait "$subpid" 2>/dev/null || true
  subpid=""
  grep -q '"n":1' "$SMOKE_TMP/a.jsonl" && ok "p2p message captured" || bad "p2p message missing"
  grep -q '"source":"smoke"' "$SMOKE_TMP/a.jsonl" \
    && ok "attachment captured" || bad "attachment missing"

  echo "== Tier B: routed network =="
  if ! command -v zenohd >/dev/null 2>&1; then
    echo "SKIP  zenohd not found — install it to enable this tier"
  else
    zenohd -l tcp/127.0.0.1:27447 \
      --cfg='scouting/multicast/address:"224.0.0.224:7546"' > "$SMOKE_TMP/zenohd.log" 2>&1 &
    zpid=$!
    sleep 2

    "$BIN" -e tcp/127.0.0.1:27447 --json sub 'zenmon-smoke/**' --duration 5s \
      > "$SMOKE_TMP/b.jsonl" 2>/dev/null &
    subpid=$!
    sleep 2
    "$BIN" -e tcp/127.0.0.1:27447 pub zenmon-smoke/routed '{"n":2}' 2>/dev/null
    wait "$subpid" 2>/dev/null || true
    subpid=""

    grep -q '"n":2' "$SMOKE_TMP/b.jsonl" \
      && ok "routed message captured" || bad "routed message missing"
    "$BIN" -e tcp/127.0.0.1:27447 --json nodes 2>/dev/null \
      | grep -q '"kind":"router"' \
      && ok "nodes lists router" || bad "nodes missing router"
    "$BIN" scout --port-range 7546-7546 --per-port-timeout 2s 2>/dev/null \
      | grep -q 'router' && ok "scout found router" || bad "scout found nothing"

    kill "$zpid" 2>/dev/null
    zpid=""
  fi

  echo
  echo "smoke: $pass passed, $fail failed"
  [ "$fail" -eq 0 ]
}

run_snapshot() {
  command -v jq >/dev/null 2>&1 || { echo "snapshot needs jq" >&2; exit 1; }
  local info nodes live
  info="$("$BIN" "$@" --json info)" || exit 1
  nodes="$("$BIN" "$@" --json nodes)" || exit 1
  live="$("$BIN" "$@" --json liveliness)" || exit 1
  jq -n --argjson info "$info" --argjson nodes "$nodes" --argjson live "$live" \
    '{info: $info, nodes: $nodes, liveliness: $live}'
}

run_capture() {
  local keyexpr="${1:?usage: driver.sh capture <keyexpr> <duration> [zenmon global flags...]}"
  local duration="${2:?usage: driver.sh capture <keyexpr> <duration> [zenmon global flags...]}"
  shift 2
  if [[ "$duration" =~ ^[0-9]+$ ]]; then duration="${duration}s"; fi
  local out="capture-$(date +%H%M%S).jsonl"

  "$BIN" "$@" capture "$keyexpr" --output "$out" --duration "$duration" || {
    local rc=$?
    [ -s "$out" ] || { echo "capture failed (exit $rc)" >&2; return "$rc"; }
  }

  local count
  count="$(wc -l < "$out" | tr -d ' ')"
  echo "captured $count messages in $duration -> $out"
  if command -v jq >/dev/null 2>&1; then
    echo "-- per-key rate --"
    local seconds
    seconds="$(awk -v d="$duration" 'BEGIN { if (d ~ /^[0-9.]+s$/) { sub(/s$/, "", d); print d } else { print 0 } }')"
    if awk -v s="$seconds" 'BEGIN { exit !(s > 0) }'; then
      jq -r '.key_expr' "$out" | sort | uniq -c | sort -rn \
        | awk -v s="$seconds" '{printf "%8.1f Hz  %6d  %s\n", $1/s, $1, $2}'
    else
      jq -r '.key_expr' "$out" | sort | uniq -c | sort -rn
    fi
  fi
}

case "$CMD" in
  smoke)    run_smoke ;;
  snapshot) run_snapshot "$@" ;;
  capture)  run_capture "$@" ;;
  *) sed -n '2,11p' "${BASH_SOURCE[0]}"; exit 2 ;;
esac
