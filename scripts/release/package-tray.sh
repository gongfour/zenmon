#!/usr/bin/env bash
#
# Registers one already-built tray artifact for the release: copies it into the
# dist directory and emits the manifest fragment that `build-manifest.sh` merges
# into the `trayArtifacts` list of zenmon.json.
#
# Unlike package-cli.sh this does no archiving of its own — the tray artifact
# is whatever the platform's installer story needs (an NSIS setup exe on
# Windows, a tar.gz of the .app on macOS), and each CI job already produced it.
# What the jobs share is only the fragment contract, so that is all this
# script owns.
set -euo pipefail

: "${VERSION:?VERSION is required (e.g. 0.1.0)}"
: "${MANIFEST_TARGET:?MANIFEST_TARGET is required (e.g. windows-x86_64)}"
: "${ARTIFACT:?ARTIFACT is required (path to the built tray artifact)}"
# What `zenmon update` acts on inside the artifact: the .app directory name in
# a tar.gz, or the setup exe itself (then equal to the artifact's file name).
: "${BINARY:?BINARY is required (e.g. zenmon-tray.app or the setup exe name)}"

OUT_DIR="${OUT_DIR:-dist}"

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d ' ' -f 1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d ' ' -f 1
  else
    echo "no sha256 tool found (need sha256sum or shasum)" >&2
    return 1
  fi
}

if [[ ! -f "$ARTIFACT" ]]; then
  echo "tray artifact not found: $ARTIFACT" >&2
  exit 1
fi

mkdir -p "$OUT_DIR"
name="$(basename "$ARTIFACT")"
if [[ ! "$ARTIFACT" -ef "${OUT_DIR}/${name}" ]]; then
  cp "$ARTIFACT" "${OUT_DIR}/${name}"
fi

sum="$(sha256_of "${OUT_DIR}/${name}")"

# The fragment name must not collide with the CLI fragment for the same
# platform, so it carries the kind in the file name as well as the field.
cat > "${OUT_DIR}/zenmon-tray-${VERSION}-${MANIFEST_TARGET}.fragment.json" <<EOF
{
  "kind": "tray",
  "target": "${MANIFEST_TARGET}",
  "url": "${name}",
  "sha256": "${sum}",
  "binary": "${BINARY}"
}
EOF

echo "registered tray artifact ${name}"
echo "  target ${MANIFEST_TARGET}"
echo "  sha256 ${sum}"
