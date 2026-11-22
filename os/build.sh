#!/bin/sh
# Build CarFluxoidOS (Buildroot ARM64 SD card image for Raspberry Pi Zero 2 W).
set -eu

BR_VER=2025.02.20
BR_SHA=fa79fa606387b3d485a6cfd30eb2302b2cc30775ffb668f2a1269ff4c9674b6b
BR_URL="https://buildroot.org/downloads/buildroot-${BR_VER}.tar.xz"
BR_TAR="/br/dl/buildroot-${BR_VER}.tar.xz"
BR_SRC="/br/src/buildroot-${BR_VER}"
BR_OUT="/br/out"
OUT_DIR="/input/local/os"

mkdir -p /br/dl /br/src "$BR_OUT" "$OUT_DIR"

if [ ! -f "$BR_TAR" ] || [ "$(sha256sum "$BR_TAR" | awk '{print $1}')" != "$BR_SHA" ]; then
 echo "Downloading Buildroot ${BR_VER}..."
 curl -fsSL "$BR_URL" -o "$BR_TAR"
fi
actual=$(sha256sum "$BR_TAR" | awk '{print $1}')
[ "$actual" = "$BR_SHA" ] || { echo "Buildroot SHA256 mismatch: $actual" >&2; exit 1; }

if [ ! -d "$BR_SRC" ]; then
 tar -xf "$BR_TAR" -C /br/src
fi

export BR2_DL_DIR=/br/dl
export FORCE_UNSAFE_CONFIGURE=1

make -C "$BR_SRC" O="$BR_OUT" BR2_EXTERNAL=/input/os carfluxoid_rpi02w_defconfig
make -C "$BR_OUT" -j"$(nproc)"

cp --sparse=never "$BR_OUT/images/carfluxoid-os.img" "$OUT_DIR/carfluxoid-os.img"
xz -T0 -6 -c "$BR_OUT/images/carfluxoid-os.img" > "$OUT_DIR/carfluxoid-os.img.xz"
(cd "$OUT_DIR" && sha256sum carfluxoid-os.img carfluxoid-os.img.xz > SHA256SUMS)
ls -lh "$OUT_DIR/carfluxoid-os.img" "$OUT_DIR/carfluxoid-os.img.xz"
