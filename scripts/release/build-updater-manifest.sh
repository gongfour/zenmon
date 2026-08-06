#!/usr/bin/env bash
#
# Builds `latest.json`, the Tauri updater manifest for zenmon-tray. The tray's
# updater plugin fetches it from the *latest* release
# (releases/latest/download/latest.json), so the url inside must be absolute
# and point at the *versioned* download — "latest" is only a stable address
# for the manifest, the artifact keeps its tagged home.
#
#   VERSION=0.1.0 REPO=gongfour/zenmon scripts/release/build-updater-manifest.sh dist
#
# The signature is the CONTENT of the .sig the Tauri CLI wrote next to the
# installer (needs TAURI_SIGNING_PRIVATE_KEY at build time) — the plugin
# refuses unsigned installs, there is no opt-out.
set -euo pipefail

: "${VERSION:?VERSION is required (e.g. 0.1.0)}"
: "${REPO:?REPO is required (e.g. gongfour/zenmon)}"

IN_DIR="${1:-dist}"

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required" >&2
  exit 1
fi

# The installer is named from the workspace version (tauri.conf.json carries no
# version on purpose — see the design doc), so a mismatch here means the guard
# job and the tray build disagree about the version. That must fail, not fuzzy-
# match to whatever setup exe is lying around.
setup="zenmon-tray_${VERSION}_x64-setup.exe"
sig="${IN_DIR}/${setup}.sig"

if [[ ! -f "${IN_DIR}/${setup}" ]]; then
  echo "missing installer: ${IN_DIR}/${setup}" >&2
  exit 1
fi
if [[ ! -f "$sig" ]]; then
  echo "missing signature: ${sig}" >&2
  echo "was the tray built without createUpdaterArtifacts / TAURI_SIGNING_PRIVATE_KEY?" >&2
  exit 1
fi

out="${IN_DIR}/latest.json"
# Same staging discipline as build-manifest.sh: never leave a half-written
# manifest that looks uploadable.
tmp="${out}.tmp"
trap 'rm -f "$tmp"' EXIT

jq -n \
  --arg version "$VERSION" \
  --arg pub_date "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --arg url "https://github.com/${REPO}/releases/download/v${VERSION}/${setup}" \
  --rawfile signature "$sig" \
  '{
     version: $version,
     pub_date: $pub_date,
     platforms: {
       "windows-x86_64": {
         url: $url,
         signature: ($signature | rtrimstr("\n") | rtrimstr("\r"))
       }
     }
   }' > "$tmp"

mv "$tmp" "$out"

echo "wrote ${out}"
jq -r '.platforms | to_entries[] | "  \(.key)  \(.value.url)"' "$out"
