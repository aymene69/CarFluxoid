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

# Build atomic A/B OS update archive (updates kernel + DTB + config + inactive SquashFS slot; never touches /data)
UPD_STAGE=$(mktemp -d)
trap 'rm -rf "$UPD_STAGE"' EXIT
cp "$BR_OUT/images/rootfs.squashfs" \
   "$BR_OUT/images/Image" \
   "$BR_OUT/images/bcm2710-rpi-zero-2-w.dtb" \
   "$BR_OUT/images/rpi-firmware/config.txt" \
   "$BR_OUT/images/rpi-firmware/cmdline.txt" \
   "$UPD_STAGE/"
mkdir -p "$UPD_STAGE/overlays"
cp -a "$BR_OUT/images/rpi-firmware/overlays/dwc2.dtbo" "$UPD_STAGE/overlays/"
(cd "$UPD_STAGE" && sha256sum rootfs.squashfs Image bcm2710-rpi-zero-2-w.dtb config.txt cmdline.txt overlays/dwc2.dtbo > SHA256SUMS)
tar -C "$UPD_STAGE" -cf "$OUT_DIR/carfluxoid-os-update.tar" \
 SHA256SUMS rootfs.squashfs Image bcm2710-rpi-zero-2-w.dtb config.txt cmdline.txt overlays

(cd "$OUT_DIR" && sha256sum carfluxoid-os.img carfluxoid-os.img.xz carfluxoid-os-update.tar > SHA256SUMS)
ls -lh "$OUT_DIR/carfluxoid-os.img" "$OUT_DIR/carfluxoid-os.img.xz" "$OUT_DIR/carfluxoid-os-update.tar"
