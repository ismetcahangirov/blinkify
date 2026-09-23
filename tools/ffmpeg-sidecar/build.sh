#!/usr/bin/env bash
# Build the Blinkify FFmpeg sidecar from source, reproducibly.
#
# Inputs, all pinned in sidecar.lock.json:
#   - the FFmpeg source, by tag and commit
#   - the cross-compilation image, by digest (BtbN/FFmpeg-Builds' win64 LGPL
#     toolchain: mingw-w64 plus every library already built as a static archive)
#   - the configure line, in configure.txt next to this script
#
# Output: dist/blinkify-ffmpeg-<version>-win64.zip containing ffmpeg.exe,
# ffprobe.exe, the LGPL licence text and a SOURCE.md naming exactly what was
# built. Upload that zip as a release asset and record its SHA-256 in
# sidecar.lock.json — see docs/engineering/ffmpeg-sidecar.md.
#
# Runs anywhere Docker runs Linux containers: a Linux CI runner, WSL, or Docker
# Desktop. It never runs in the pull-request gate; building FFmpeg takes far
# longer than the 15-minute budget, and the result is a pinned artefact.

set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
LOCK="$HERE/sidecar.lock.json"
OUT="$HERE/dist"

json() { node -e "process.stdout.write(String(require(process.argv[1]).$1))" "$LOCK"; }

FFMPEG_TAG="$(json source.tag)"
FFMPEG_COMMIT="$(json source.commit)"
IMAGE="$(json toolchain.image)@$(json toolchain.digest)"
BUILD_NAME="blinkify-ffmpeg-$(json version)-win64"

# The configure line: every non-comment, non-blank line of configure.txt.
FLAGS="$(grep -v '^\s*#' "$HERE/configure.txt" | sed 's/#.*//' | tr -s ' \t\r\n' ' ')"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

cat >"$WORK/build-inside.sh" <<EOF
set -euxo pipefail
# Built on the container's own filesystem, not the mount: on Docker Desktop a
# bind mount turns ten thousand small reads and writes into a very long wait.
mkdir -p /tmp/build
cd /tmp/build
git clone --depth 1 --branch '$FFMPEG_TAG' https://github.com/FFmpeg/FFmpeg.git ffmpeg
cd ffmpeg
# A tag can be moved; a commit cannot. Refuse to build anything else.
test "\$(git rev-parse HEAD)" = '$FFMPEG_COMMIT'

# The image supplies the cross toolchain and the flags that locate its static
# libraries (FFBUILD_TARGET_FLAGS, FF_CFLAGS, FF_LIBS ...). What gets enabled is
# ours: FF_CONFIGURE from the image is deliberately not used.
./configure --prefix=/tmp/build/prefix --pkg-config-flags=--static \
    \$FFBUILD_TARGET_FLAGS $FLAGS \
    --extra-cflags="\$FF_CFLAGS" --extra-cxxflags="\$FF_CXXFLAGS" \
    --extra-libs="\$FF_LIBS" --extra-ldflags="\$FF_LDFLAGS" \
    --extra-ldexeflags="\$FF_LDEXEFLAGS" \
    --cc="\$CC" --cxx="\$CXX" --ar="\$AR" --ranlib="\$RANLIB" --nm="\$NM" \
    --extra-version=blinkify \
  || { tail -n 200 ffbuild/config.log; exit 1; }
make -j"\$(nproc)"
make install

PKG=/tmp/build/pkg/$BUILD_NAME
mkdir -p "\$PKG" /work/out
cp /tmp/build/prefix/bin/ffmpeg.exe /tmp/build/prefix/bin/ffprobe.exe "\$PKG/"
cp COPYING.LGPLv2.1 "\$PKG/LICENSE.txt"
cat >"\$PKG/SOURCE.md" <<'SOURCE'
# Blinkify FFmpeg sidecar — corresponding source

These binaries are FFmpeg $FFMPEG_TAG, licensed under the GNU Lesser General
Public License version 2.1 or later (LICENSE.txt).

- Source: https://github.com/FFmpeg/FFmpeg, commit $FFMPEG_COMMIT (tag $FFMPEG_TAG)
- Toolchain image: $IMAGE
- Build script and configure line:
  https://github.com/ismetcahangirov/blinkify/tree/main/tools/ffmpeg-sidecar

You may replace these files with your own build of FFmpeg. Blinkify invokes
them as separate processes and does not link them.
SOURCE
# Fixed timestamps and no extra attributes, so the zip's own hash depends only
# on its contents.
find /tmp/build/pkg -exec touch -h -d '2000-01-01T00:00:00Z' {} +
(cd /tmp/build/pkg && zip -X -9 -r /work/out/$BUILD_NAME.zip $BUILD_NAME)
sha256sum "\$PKG/ffmpeg.exe" "\$PKG/ffprobe.exe" /work/out/$BUILD_NAME.zip
EOF

# As the calling user, so the files it leaves in $WORK can be cleaned up without
# root on a Linux host. Docker Desktop on Windows maps ownership itself.
USER_ARGS=()
if [[ "$(uname -s)" == Linux ]]; then
  USER_ARGS=(-u "$(id -u):$(id -g)")
fi
# Git Bash on Windows rewrites anything that looks like a POSIX path in an
# argument, including the container-side ones; Docker Desktop wants a Windows
# path for the host side of the mount.
WORK_HOST="$WORK"
if command -v cygpath >/dev/null 2>&1; then
  export MSYS_NO_PATHCONV=1
  WORK_HOST="$(cygpath -m "$WORK")"
fi
docker run --rm "${USER_ARGS[@]}" -e HOME=/tmp -v "$WORK_HOST":/work "$IMAGE" bash /work/build-inside.sh

mkdir -p "$OUT"
cp "$WORK/out/$BUILD_NAME.zip" "$OUT/"

echo
echo "Built $OUT/$BUILD_NAME.zip"
