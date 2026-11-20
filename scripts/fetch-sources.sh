#!/usr/bin/env bash
#
# Download the exact FFmpeg and libx264 sources that the release binaries are
# built from, and verify them against the pinned checksums.
#
#   scripts/fetch-sources.sh <output-directory>
#
# The downloaded archives are the "complete corresponding source code" that the
# GPL requires us to ship next to the binaries.

set -euo pipefail

FFMPEG_VERSION="9.0.2"
FFMPEG_SHA256="8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e"
FFMPEG_URL="https://ffmpeg.org/releases/ffmpeg-${FFMPEG_VERSION}.tar.xz"

X264_COMMIT="31e19f92f00c7003fa115047ce50978bc98c3a0d"
X264_SHA256="d053c9d86988d6bc78237ca5205865c5ddf99c98ef4cd9927eec8f6d388f6dd9"
X264_URL="https://github.com/mirror/x264/archive/${X264_COMMIT}.tar.gz"

OUT_DIR="${1:?output directory required}"

mkdir -p "$OUT_DIR"

verify() {
  local file="$1" expected="$2" actual
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$file" | awk '{print $1}')"
  else
    actual="$(shasum -a 256 "$file" | awk '{print $1}')"
  fi
  if [ "$actual" != "$expected" ]; then
    echo "checksum mismatch for $(basename "$file"):" >&2
    echo "  expected $expected" >&2
    echo "  actual   $actual" >&2
    exit 1
  fi
  echo "verified $(basename "$file")"
}

fetch() {
  local url="$1" file="$2" expected="$3"
  if [ ! -f "$file" ]; then
    echo "downloading $url"
    curl -fsSL --retry 3 -o "$file" "$url"
  fi
  verify "$file" "$expected"
}

fetch "$FFMPEG_URL" "$OUT_DIR/ffmpeg-${FFMPEG_VERSION}.tar.xz" "$FFMPEG_SHA256"
fetch "$X264_URL" "$OUT_DIR/x264-${X264_COMMIT}.tar.gz" "$X264_SHA256"

# The license texts travel with the FFmpeg sources; extract the ones that apply
# to our configuration (--enable-gpl --enable-version3).
if [ ! -f "$OUT_DIR/COPYING.GPLv3" ]; then
  echo "extracting the license texts"
  tmp="$(mktemp -d)"
  tar -xf "$OUT_DIR/ffmpeg-${FFMPEG_VERSION}.tar.xz" -C "$tmp"
  cp "$tmp"/ffmpeg-${FFMPEG_VERSION}/COPYING.GPLv3 "$OUT_DIR/COPYING.GPLv3"
  cp "$tmp"/ffmpeg-${FFMPEG_VERSION}/COPYING.LGPLv3 "$OUT_DIR/COPYING.LGPLv3"
fi

echo "sources are ready in $OUT_DIR"
