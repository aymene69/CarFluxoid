#!/bin/sh
# Buildroot post-image hook: assemble 3-partition CarFluxoidOS SD image (boot + RO SquashFS + /data ext4).
set -eu

BOARD_DIR="$BR2_EXTERNAL_CARFLUXOID_PATH/board/rpi02w"
GENIMAGE_TMP="${BUILD_DIR}/genimage.tmp"
ROOTPATH_TMP=$(mktemp -d)
trap 'rm -rf "$ROOTPATH_TMP"' EXIT

rm -rf "$GENIMAGE_TMP"

genimage \
 --rootpath "$ROOTPATH_TMP" \
 --tmppath "$GENIMAGE_TMP" \
 --inputpath "$BINARIES_DIR" \
 --outputpath "$BINARIES_DIR" \
 --config "$BOARD_DIR/genimage.cfg"

echo "CarFluxoidOS SD image built: $BINARIES_DIR/carfluxoid-os.img"
