#!/usr/bin/env bash
#
# Assemble the release archive for one target.
#
#   scripts/package-release.sh <target> <archive-format>
#
# It expects two directories in the working directory:
#   app/    the compiled application binary
#   ffmpeg/ ffmpeg, ffprobe and the FFmpeg license texts
#
# and writes the finished archive plus its checksum into dist/.

set -euo pipefail

TARGET="${1:?target required}"
FORMAT="${2:?archive format required (tar.gz or zip)}"

DIST="dist"
STAGE="pkg"
PKG="${STAGE}/lossless-vidcut-${TARGET}"

EXE="lossless-vidcut"
FF_EXE=""
case "$TARGET" in
  windows-x86_64)
    EXE="lossless-vidcut.exe"
    FF_EXE=".exe"
    ;;
esac

mkdir -p "$PKG" "$DIST"

cp "app/${EXE}" "$PKG/${EXE}"
cp "ffmpeg/ffmpeg${FF_EXE}" "$PKG/ffmpeg${FF_EXE}"
cp "ffmpeg/ffprobe${FF_EXE}" "$PKG/ffprobe${FF_EXE}"

# The artifact download does not keep the permission bits, so the programs are
# marked as executable again here.
chmod +x "$PKG/${EXE}" "$PKG/ffmpeg${FF_EXE}" "$PKG/ffprobe${FF_EXE}"
chmod 644 "$PKG/README.md" "$PKG/LICENSE" "$PKG/COPYING.GPLv3" "$PKG/COPYING.LGPLv3" \
  "$PKG/configure-line.txt" "$PKG/FFMPEG-NOTICE.txt"
cp "ffmpeg/COPYING.GPLv3" "$PKG/COPYING.GPLv3"
cp "ffmpeg/COPYING.LGPLv3" "$PKG/COPYING.LGPLv3"
cp README.md "$PKG/README.md"
cp LICENSE "$PKG/LICENSE"

FFMPEG_VERSION="9.0.2"
X264_COMMIT="31e19f92f00c7003fa115047ce50978bc98c3a0d"

cat > "$PKG/FFMPEG-NOTICE.txt" <<EOF
FFmpeg notice
=============

This distribution of Lossless VidCut ships the FFmpeg command line tools
(ffmpeg and ffprobe) next to the application binary. The application starts
them as separate child processes and talks to them through pipes, so the two
programs stay independent of each other.

FFmpeg version : ${FFMPEG_VERSION}
libx264        : ${X264_COMMIT}
Target         : ${TARGET}

The binaries are statically linked and contain no changes made by the authors
of Lossless VidCut.

FFmpeg is free software licensed under the GNU Lesser General Public License
version 2.1 or later. This build enables the GPL components (--enable-gpl) and
version 3 of the license (--enable-version3), therefore the shipped binaries
are licensed under the GNU General Public License version 3 or later.

The complete corresponding source code of the FFmpeg and libx264 sources used
for this build is published as the release asset
"lossless-vidcut-ffmpeg-sources.tar.xz" and is available at these addresses:

  FFmpeg : https://ffmpeg.org/releases/ffmpeg-${FFMPEG_VERSION}.tar.xz
  libx264: https://github.com/mirror/x264/archive/${X264_COMMIT}.tar.gz

The exact configure lines that were used are in the file
"configure-line.txt" next to this notice. The license texts of both projects
are included as COPYING.GPLv3 and COPYING.LGPLv3.
EOF

cp "ffmpeg/configure-line.txt" "$PKG/configure-line.txt"

cd "$STAGE"
if [ "$FORMAT" = "zip" ]; then
  zip -r -q "../${DIST}/lossless-vidcut-${TARGET}.zip" "lossless-vidcut-${TARGET}"
else
  tar -czf "../${DIST}/lossless-vidcut-${TARGET}.tar.gz" "lossless-vidcut-${TARGET}"
fi
cd ..

( cd "$DIST" && sha256sum "lossless-vidcut-${TARGET}.${FORMAT}" > "lossless-vidcut-${TARGET}.${FORMAT}.sha256" )

echo "== package =="
ls -lh "$DIST"
