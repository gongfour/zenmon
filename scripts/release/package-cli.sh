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

# Absolute form of a path whose file may not exist yet (so `realpath -e` and
# `cd $(dirname)` on a missing directory are both out).
absolute_path() {
  case "$1" in
    /* | [A-Za-z]:[/\\]*) printf '%s\n' "$1" ;;
    *) printf '%s\n' "$PWD/$1" ;;
  esac
}

# Windows-native spelling, when running under MSYS/Git Bash/Cygwin. A no-op
# everywhere else, so the same call site works on Linux and macOS.
native_path() {
  if command -v cygpath >/dev/null 2>&1; then
    cygpath -w "$1"
  else
    printf '%s\n' "$1"
  fi
}

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
    # Both zip tools are native Windows programs run from a POSIX shell, so
    # every path handed to them goes through `native_path`: MSYS/Git Bash
    # spells the repo root `/d/a/...`, which `7z.exe` and PowerShell resolve
    # against the current *drive* instead (`D:\d\a\...`) and then report as
    # "does not exist".
    if command -v 7z >/dev/null 2>&1; then
      # 7z runs from inside the staging directory so the binary lands at the
      # archive root; that makes a relative destination resolve against the
      # staging directory, so it has to be absolute first.
      archive_dest="$(native_path "$(absolute_path "$archive")")"
      (cd "$STAGE_DIR" && 7z a -tzip -bso0 -bsp0 "$archive_dest" "${BINARY}")
    elif command -v powershell >/dev/null 2>&1; then
      stage_src="$(native_path "$(absolute_path "$STAGE_DIR")")"
      archive_dest="$(native_path "$(absolute_path "$archive")")"
      powershell -NoProfile -NonInteractive -Command \
        "Compress-Archive -Path '${stage_src}\\*' -DestinationPath '${archive_dest}' -Force"
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
