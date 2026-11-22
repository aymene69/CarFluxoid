#!/bin/sh
# Buildroot post-build hook for CarFluxoidOS (Raspberry Pi Zero 2 W).
set -eu

TARGET_DIR=$1
REPO_ROOT=$(CDPATH= cd -- "$BR2_EXTERNAL_CARFLUXOID_PATH/.." && pwd)
BINS="$REPO_ROOT/local/raspberrypi"

[ -x "$BINS/aa-phone" ] || { echo "Missing $BINS/aa-phone — run 'make build' first" >&2; exit 1; }
[ -x "$BINS/catplay-aa-rx" ] || { echo "Missing $BINS/catplay-aa-rx — run 'make build' first" >&2; exit 1; }

# Ensure executable permissions on overlay init scripts and sbin helpers
chmod 755 "$TARGET_DIR"/etc/init.d/S* "$TARGET_DIR"/usr/sbin/carfluxoid-*

# Remove default Buildroot init scripts superseded by our deterministic S* services.
rm -f "$TARGET_DIR/etc/init.d/S40network" \
      "$TARGET_DIR/etc/init.d/S80dnsmasq" \
      "$TARGET_DIR/etc/init.d/S50hostapd" \
      "$TARGET_DIR/etc/init.d/S30dbus" \
      "$TARGET_DIR/etc/init.d/S30dbus-daemon" \
      "$TARGET_DIR/etc/init.d/S40bluetoothd" \
      "$TARGET_DIR/etc/init.d/S50crond"

# Base CarFluxoid tree lives in immutable SquashFS at /usr/share/carfluxoid and is
# overlaid onto /opt/carfluxoid at boot (backed by /data/overlay/opt/upper).
BASE="$TARGET_DIR/usr/share/carfluxoid"
install -d -m 755 "$BASE/bin" "$BASE/assets" "$BASE/tls" "$TARGET_DIR/opt/carfluxoid" "$TARGET_DIR/data" "$TARGET_DIR/mnt/boot"

install -m 755 "$BINS/aa-phone" "$BASE/bin/aa-phone"
install -m 755 "$BINS/catplay-aa-rx" "$BASE/bin/catplay-aa-rx"
install -m 755 "$REPO_ROOT/platforms/raspberrypi/dashboard/dashboard.py" "$BASE/bin/dashboard.py"
ln -sf /usr/sbin/carfluxoid-start-ap "$BASE/bin/start-ap-trial.sh"
ln -sf /usr/sbin/carfluxoid-start-radio "$BASE/bin/start-radio.sh"
ln -sf /usr/sbin/carfluxoid-diag "$BASE/bin/car-diag.sh"

install -m 644 "$REPO_ROOT/assets/mire.h264" "$BASE/assets/mire.h264"
install -m 644 "$REPO_ROOT/assets/waiting"/attente-*.h264 "$BASE/assets/"
install -m 644 "$REPO_ROOT/tooling/waiting_iphone_config.py" "$BASE/assets/waiting_iphone_config.py"

# Optional: bundle reference Android Auto TLS pair if already fetched locally (never committed to git)
if [ -s "$REPO_ROOT/local/tls/test-cert.pem" ] && [ -s "$REPO_ROOT/local/tls/test-key.pem" ]; then
 install -m 600 "$REPO_ROOT/local/tls/test-cert.pem" "$BASE/tls/test-cert.pem"
 install -m 600 "$REPO_ROOT/local/tls/test-key.pem" "$BASE/tls/test-key.pem"
fi

# Persistent symlinks into /data (writable ext4 partition); /var/lib/dbus is already -> /tmp/dbus
mkdir -p "$TARGET_DIR/var/lib" "$TARGET_DIR/root"
rm -rf "$TARGET_DIR/var/lib/carfluxoid" "$TARGET_DIR/var/lib/bluetooth" "$TARGET_DIR/etc/dropbear" "$TARGET_DIR/root/.ssh"
ln -sf /data "$TARGET_DIR/var/lib/carfluxoid"
ln -sf /data/bluetooth "$TARGET_DIR/var/lib/bluetooth"
ln -sf /data/dropbear "$TARGET_DIR/etc/dropbear"
ln -sf /data/dropbear "$TARGET_DIR/root/.ssh"

# Font path compatibility with dashboard.py (/usr/share/fonts/truetype/dejavu)
if [ -d "$TARGET_DIR/usr/share/fonts/dejavu" ]; then
 mkdir -p "$TARGET_DIR/usr/share/fonts/truetype"
 ln -sf /usr/share/fonts/dejavu "$TARGET_DIR/usr/share/fonts/truetype/dejavu"
fi

# Verify every DT_NEEDED shared library of aa-phone & catplay-aa-rx exists in TARGET_DIR
for bin in "$BASE/bin/aa-phone" "$BASE/bin/catplay-aa-rx"; do
 for lib in $(readelf -d "$bin" | sed -n 's/.*Shared library: \[\(.*\)\]/\1/p'); do
  if [ ! -e "$TARGET_DIR/lib/$lib" ] && [ ! -e "$TARGET_DIR/usr/lib/$lib" ] && [ ! -e "$TARGET_DIR/lib64/$lib" ] && [ ! -e "$TARGET_DIR/usr/lib64/$lib" ]; then
   echo "ERROR: missing shared library $lib required by $bin" >&2
   exit 1
  fi
 done
done
# Verify kmod/libkmod supports .ko.xz when kernel modules are XZ-compressed
if find "$TARGET_DIR/lib/modules" -name '*.ko.xz' | grep -q .; then
 if ! readelf -d "$TARGET_DIR/usr/bin/kmod" "$TARGET_DIR"/usr/lib/libkmod.so* 2>/dev/null | grep -q 'liblzma\.so'; then
  echo "ERROR: kernel modules are .ko.xz but target kmod was built without liblzma (enable BR2_PACKAGE_XZ=y)" >&2
  exit 1
 fi
fi

echo "CarFluxoidOS post-build verified: all shared libraries and kmod XZ support present."
