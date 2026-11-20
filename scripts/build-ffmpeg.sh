#!/usr/bin/env bash
#
# Build a self-contained FFmpeg (ffmpeg + ffprobe) for one release target.
#
#   scripts/build-ffmpeg.sh <target> <output-directory>
#
# Targets:
#   linux-x86_64    native build
#   windows-x86_64  cross compiled with mingw-w64
#   macos-aarch64   native build on Apple Silicon
#   macos-x86_64    cross compiled on Apple Silicon
#
# The binaries are static, so the release archive works on a clean machine
# without an FFmpeg installation. x264 is built from source as well, because it
# is the encoder the precise cutting mode uses.

set -euo pipefail

FFMPEG_VERSION="9.0.2"
FFMPEG_SHA256="8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e"
X264_COMMIT="31e19f92f00c7003fa115047ce50978bc98c3a0d"
X264_SHA256="d053c9d86988d6bc78237ca5205865c5ddf99c98ef4cd9927eec8f6d388f6dd9"

TARGET="${1:?target required}"
OUT_DIR="${2:?output directory required}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK_DIR="$(mktemp -d)"
PREFIX="${WORK_DIR}/prefix"

JOBS="$( (command -v nproc >/dev/null 2>&1 && nproc) || sysctl -n hw.ncpu || echo 2)"

EXE=""
case "$TARGET" in
  windows-x86_64) EXE=".exe" ;;
esac

X264_CONFIG=()
FFMPEG_CONFIG=()
export CFLAGS_EXTRA=""
export LDFLAGS_EXTRA=""

case "$TARGET" in
  linux-x86_64)
    X264_CONFIG=(
      --prefix="$PREFIX" --enable-static --disable-shared --disable-cli --enable-pic
    )
    FFMPEG_CONFIG=(
      --prefix="$PREFIX" --disable-shared --enable-static --disable-doc --disable-debug
      --enable-gpl --enable-version3 --enable-libx264 --pkg-config-flags=--static
      --extra-cflags="-I${PREFIX}/include" --extra-ldflags="-L${PREFIX}/lib"
    )
    ;;
  windows-x86_64)
    export CROSS_PREFIX="x86_64-w64-mingw32-"
    # FFmpeg prefixes every tool with the cross prefix, including pkg-config.
    # There is no such binary, and the one we need is the ordinary pkg-config:
    # it only has to find the libx264 we just built.
    mkdir -p "${WORK_DIR}/bin"
    printf '#!/bin/sh\nexec pkg-config "$@"\n' > "${WORK_DIR}/bin/x86_64-w64-mingw32-pkg-config"
    chmod +x "${WORK_DIR}/bin/x86_64-w64-mingw32-pkg-config"
    export PATH="${WORK_DIR}/bin:${PATH}"
    X264_CONFIG=(
      --cross-prefix=x86_64-w64-mingw32- --host=x86_64-w64-mingw32
      --prefix="$PREFIX" --enable-static --disable-shared --disable-cli --enable-pic
    )
    FFMPEG_CONFIG=(
      --target-os=win32 --arch=x86_64 --cross-prefix=x86_64-w64-mingw32- --prefix="$PREFIX"
      --disable-shared --enable-static --disable-doc --disable-debug
      --enable-gpl --enable-version3 --enable-libx264 --pkg-config-flags=--static
      --extra-cflags="-I${PREFIX}/include" --extra-ldflags="-L${PREFIX}/lib -static"
    )
    ;;
  macos-aarch64)
    export MACOSX_DEPLOYMENT_TARGET="11.0"
    X264_CONFIG=(
      --prefix="$PREFIX" --enable-static --disable-shared --disable-cli --enable-pic
    )
    FFMPEG_CONFIG=(
      --prefix="$PREFIX" --disable-shared --enable-static --disable-doc --disable-debug
      --enable-gpl --enable-version3 --enable-libx264 --pkg-config-flags=--static
      --extra-cflags="-I${PREFIX}/include" --extra-ldflags="-L${PREFIX}/lib"
    )
    ;;
  macos-x86_64)
    export MACOSX_DEPLOYMENT_TARGET="11.0"
    export CFLAGS_EXTRA="-arch x86_64"
    export LDFLAGS_EXTRA="-arch x86_64"
    X264_CONFIG=(
      --arch=x86_64 --prefix="$PREFIX"
      --enable-static --disable-shared --disable-cli --enable-pic
    )
    FFMPEG_CONFIG=(
      --arch=x86_64 --prefix="$PREFIX" --disable-shared --enable-static
      --disable-doc --disable-debug
      --enable-gpl --enable-version3 --enable-libx264 --pkg-config-flags=--static
      --extra-cflags="-I${PREFIX}/include -arch x86_64"
      --extra-ldflags="-L${PREFIX}/lib -arch x86_64"
    )
    ;;
  *)
    echo "unknown target: $TARGET" >&2
    exit 1
    ;;
esac

echo "== target $TARGET, $JOBS jobs =="

# ---------------------------------------------------------------- sources ---
bash "${SCRIPT_DIR}/fetch-sources.sh" "${WORK_DIR}/sources"

tar -xf "${WORK_DIR}/sources/ffmpeg-${FFMPEG_VERSION}.tar.xz" -C "$WORK_DIR"
tar -xf "${WORK_DIR}/sources/x264-${X264_COMMIT}.tar.gz" -C "$WORK_DIR"

X264_DIR="${WORK_DIR}/x264-${X264_COMMIT}"
FFMPEG_DIR="${WORK_DIR}/ffmpeg-${FFMPEG_VERSION}"

# ------------------------------------------------------------------- x264 ---
(
  cd "$X264_DIR"
  echo "== configuring x264 =="
  ./configure "${X264_CONFIG[@]}"
  echo "== building x264 =="
  make -j"$JOBS"
  make install
)

# ----------------------------------------------------------------- ffmpeg ---
# The freshly built libx264 lives in our own prefix, so pkg-config has to look
# there as well.
export PKG_CONFIG_PATH="${PREFIX}/lib/pkgconfig:${PKG_CONFIG_PATH:-}"

(
  cd "$FFMPEG_DIR"
  echo "== configuring ffmpeg =="
  # shellcheck disable=SC2086
  env CFLAGS="${CFLAGS_EXTRA:-} ${CFLAGS:-}" LDFLAGS="${LDFLAGS_EXTRA:-} ${LDFLAGS:-}" \
    ./configure "${FFMPEG_CONFIG[@]}"
  echo "== building ffmpeg =="
  make -j"$JOBS"
  make install
)

# ----------------------------------------------------------------- output ---
mkdir -p "$OUT_DIR"
cp "${PREFIX}/bin/ffmpeg${EXE}" "$OUT_DIR/ffmpeg${EXE}"
cp "${PREFIX}/bin/ffprobe${EXE}" "$OUT_DIR/ffprobe${EXE}"
cp "${WORK_DIR}/sources/COPYING.GPLv3" "$OUT_DIR/COPYING.GPLv3"
cp "${WORK_DIR}/sources/COPYING.LGPLv3" "$OUT_DIR/COPYING.LGPLv3"

# Remember the exact configure line for the license notice.
{
  printf '%s' "libx264: "
  printf '%s ' "${X264_CONFIG[@]}"
  printf '\n%s' "ffmpeg:   "
  printf '%s ' "${FFMPEG_CONFIG[@]}"
  printf '\n'
} > "$OUT_DIR/configure-line.txt"

echo "== result =="
ls -l "$OUT_DIR"
"${OUT_DIR}/ffmpeg${EXE}" -version | head -n 3
