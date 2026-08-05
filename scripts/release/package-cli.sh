#!/usr/bin/env bash
#
# Packages one already-built `zenmon` binary into its release archive and emits
# the manifest fragment that `build-manifest.sh` later merges into zenmon.json.
#
# Run once per platform, from the repository root, after
# `cargo build --release -p zenmon-cli --target $TARGET`.
#
# The fragment carries the manifest `target` string explicitly rather than
# letting the merge step derive it from the file name: the manifest target is
# `{env::consts::OS}-{env::consts::ARCH}` (what the running binary reports about
# itself), which is not a substring of the Rust target triple. Deriving one from
# the other would be a lookup table in two places that can silently disagree.
set -euo pipefail

: "${VERSION:?VERSION is required (e.g. 0.1.0)}"
: "${TARGET:?TARGET is required (Rust target triple)}"
: "${MANIFEST_TARGET:?MANIFEST_TARGET is required (e.g. windows-x86_64)}"
: "${BINARY:?BINARY is required (e.g. zenmon or zenmon.exe)}"
: "${ARCHIVE_FORMAT:?ARCHIVE_FORMAT is required (zip or tar.gz)}"

OUT_DIR="${OUT_DIR:-dist}"

# Fixed, not overridable: this directory is `rm -rf`'d, and an env-supplied path
# would put an arbitrary target under that.
STAGE_DIR=".release-stage"

sha256_of() {
  # sha256sum on Linux and Git Bash for Windows; macOS ships shasum instead.
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d ' ' -f 1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d ' ' -f 1
  else
    echo "no sha256 tool found (need sha256sum or shasum)" >&2
    return 1
  fi
}

src="target/${TARGET}/release/${BINARY}"
if [[ ! -f "$src" ]]; then
  echo "built binary not found: $src" >&2
  echo "run: cargo build --release -p zenmon-cli --target ${TARGET}" >&2
  exit 1
fi

rm -rf "$STAGE_DIR"
mkdir -p "$STAGE_DIR" "$OUT_DIR"
cp "$src" "${STAGE_DIR}/${BINARY}"

base="zenmon-${VERSION}-${TARGET}"

case "$ARCHIVE_FORMAT" in
  zip)
    archive="${OUT_DIR}/${base}.zip"
    rm -f "$archive"
    # 7z ships on GitHub's windows runners; Compress-Archive is the guaranteed
    # fallback on any Windows box. Both write the binary at the archive root.
    if command -v 7z >/dev/null 2>&1; then
      (cd "$STAGE_DIR" && 7z a -tzip -bso0 -bsp0 "../${archive}" "${BINARY}")
    elif command -v powershell >/dev/null 2>&1; then
      powershell -NoProfile -NonInteractive -Command \
        "Compress-Archive -Path '${STAGE_DIR}/*' -DestinationPath '${archive}' -Force"
    else
      echo "no zip tool found (need 7z or powershell)" >&2
      exit 1
    fi
    ;;
  tar.gz)
    archive="${OUT_DIR}/${base}.tar.gz"
    rm -f "$archive"
    tar czf "$archive" -C "$STAGE_DIR" "${BINARY}"
    ;;
  *)
    echo "unknown ARCHIVE_FORMAT: ${ARCHIVE_FORMAT} (expected zip or tar.gz)" >&2
    exit 1
    ;;
esac

sum="$(sha256_of "$archive")"

# `url` is deliberately a bare file name — a manifest-relative reference. It
# resolves inside a GitHub release and equally inside a folder copied to a USB
# stick, so the same file set works for every remote kind without a rewrite.
cat > "${OUT_DIR}/${base}.fragment.json" <<EOF
{
  "target": "${MANIFEST_TARGET}",
  "url": "$(basename "$archive")",
  "sha256": "${sum}",
  "binary": "${BINARY}"
}
EOF

rm -rf "$STAGE_DIR"

echo "packaged ${archive}"
echo "  target ${MANIFEST_TARGET}"
echo "  sha256 ${sum}"
