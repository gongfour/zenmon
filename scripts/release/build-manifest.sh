#!/usr/bin/env bash
#
# Merges the per-platform fragments written by `package-cli.sh` and
# `package-tray.sh` into the single `zenmon.json` release manifest that
# `zenmon update` reads.
#
#   VERSION=0.1.0 scripts/release/build-manifest.sh dist
#
# Schema (v1):
#   { schemaVersion, version, channel,
#     artifacts:     [{ target, url, sha256, binary }],
#     trayArtifacts: [{ target, url, sha256, binary }] }
#
# Fragments carry a `kind` (cli | tray, absent means cli) that routes them into
# `artifacts` or `trayArtifacts`; the field itself is stripped from the output,
# the array membership already says it. Tray entries live in their own array —
# not in `artifacts` with a kind tag — because the already-shipped v0.1.0
# updater selects from `artifacts` by target alone and would happily install a
# tray archive as the CLI. schemaVersion stays 1: an unknown extra field is
# ignored by old updaters, while a version bump would make them refuse the
# whole manifest and break their update path.
set -euo pipefail

: "${VERSION:?VERSION is required (e.g. 0.1.0)}"

IN_DIR="${1:-dist}"
CHANNEL="${CHANNEL:-stable}"

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required" >&2
  exit 1
fi

shopt -s nullglob
fragments=("$IN_DIR"/*.fragment.json)
if (( ${#fragments[@]} == 0 )); then
  echo "no manifest fragments found in ${IN_DIR}" >&2
  echo "each platform's package-cli.sh run must land its fragment here" >&2
  exit 1
fi

# jq.exe opens stdout in text mode on Windows, so every `jq -r` line arrives
# with a trailing CR. CI runs this on ubuntu where that does not happen, but the
# script is meant to be runnable locally while debugging a release — and a CR
# glued onto a file name turns the existence check below into a false alarm.
jq_lines() {
  jq -r "$@" | tr -d '\r'
}

# A duplicate kind+target means two build jobs claimed the same platform, and
# the updater would silently take whichever sorted first. Fail loudly instead —
# a release that ships the wrong binary for a platform is worse than no
# release. Keyed on kind+target, not target alone: the CLI and the tray for
# one platform are the expected case, two tray builds for it are not.
duplicates="$(jq_lines '"\(.kind // "cli") \(.target)"' "${fragments[@]}" | sort | uniq -d)"
if [[ -n "$duplicates" ]]; then
  echo "duplicate kind+target across fragments:" >&2
  echo "$duplicates" >&2
  exit 1
fi

out="${IN_DIR}/zenmon.json"
# Built under a temporary name and moved into place only once it validates, so a
# failed run never leaves a plausible-looking zenmon.json for someone to pick up
# and upload by hand.
tmp="${out}.tmp"
trap 'rm -f "$tmp"' EXIT

# `trayArtifacts` is omitted entirely when no tray fragment exists, so a
# CLI-only release produces a byte-identical manifest to what it did before
# the field was introduced.
jq -s \
  --arg version "$VERSION" \
  --arg channel "$CHANNEL" \
  '{
     schemaVersion: 1,
     version: $version,
     channel: $channel,
     artifacts: [.[] | select((.kind // "cli") == "cli") | del(.kind)] | sort_by(.target)
   }
   + (([.[] | select(.kind == "tray") | del(.kind)] | sort_by(.target)) as $tray
      | if $tray == [] then {} else {trayArtifacts: $tray} end)' \
  "${fragments[@]}" > "$tmp"

# Every artifact the manifest names must actually be sitting next to it, or the
# release publishes a manifest pointing at a download that 404s.
missing=0
while IFS= read -r url; do
  if [[ ! -f "${IN_DIR}/${url}" ]]; then
    echo "manifest references a missing artifact: ${url}" >&2
    missing=1
  fi
done < <(jq_lines '(.artifacts + (.trayArtifacts // []))[].url' "$tmp")
(( missing == 0 )) || exit 1

mv "$tmp" "$out"

echo "wrote ${out}"
jq_lines '(.artifacts[] | "  cli   \(.target)  \(.url)"),
          ((.trayArtifacts // [])[] | "  tray  \(.target)  \(.url)")' "$out"
