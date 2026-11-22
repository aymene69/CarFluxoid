#!/bin/sh
# Build CarFluxoid ARM64 binaries and OTA update bundle inside an ARM64 Linux container.
set -eu
[ "$(uname -m)" = aarch64 ] || { echo 'ARM64 Linux build environment required' >&2; exit 1; }

out=/input/local/raspberrypi
bundle="$out/update-bundle"
host=/build/pi/mbedtls
mkdir -p "$out" /build/pi

if [ ! -f "$host/Makefile" ];then
 python3 - <<'PREPARE'
from pathlib import Path
import hashlib, json, tarfile, urllib.request
spec = json.loads(Path('/input/tooling/sources.lock.json').read_text())['sources']['mbedtls']
cache = Path('/build/pi/mbedtls.tar.gz')
if not cache.exists() or hashlib.sha256(cache.read_bytes()).hexdigest() != spec['sha256']:
 with urllib.request.urlopen(spec['url'], timeout=120) as response:
  cache.write_bytes(response.read())
assert hashlib.sha256(cache.read_bytes()).hexdigest() == spec['sha256']
dest = Path('/build/pi/mbedtls-extract')
dest.mkdir(exist_ok=True)
with tarfile.open(cache) as archive:
 archive.extractall(dest, filter='data')
next(dest.iterdir()).rename('/build/pi/mbedtls')
PREPARE
fi

pi_cflags='-O3 -mcpu=cortex-a53'
if [ ! -f "$host/.carfluxoid-cflags" ] || [ "$(cat "$host/.carfluxoid-cflags")" != "$pi_cflags" ];then
 make -C "$host" clean
 printf '%s' "$pi_cflags" > "$host/.carfluxoid-cflags"
fi
make -C "$host" -j4 CFLAGS="$pi_cflags" lib

cc $pi_cflags -DSYS_io_pgetevents_time64=SYS_io_pgetevents \
 -I"$host/include" /input/bridge-aa/src/aa_phone.c \
 "$host/library/libmbedtls.a" "$host/library/libmbedx509.a" "$host/library/libmbedcrypto.a" \
 -lpthread -o "$out/aa-phone"

export CARGO_TARGET_DIR=/build/pi/target
export OPENSSL_NO_VENDOR=1
cargo build --release --locked --manifest-path /input/bridge-aa/carplay-rx/Cargo.toml
cp "$CARGO_TARGET_DIR/release/catplay-aa-rx" "$out/catplay-aa-rx"
strip "$out/aa-phone" "$out/catplay-aa-rx"
"$out/catplay-aa-rx" --self-test

rm -rf "$bundle"
mkdir -p "$bundle"
cp "$out/aa-phone" "$out/catplay-aa-rx" "$bundle/"
cp /input/platforms/raspberrypi/start-radio.sh \
   /input/platforms/raspberrypi/start-ap-trial.sh \
   /input/platforms/raspberrypi/dashboard/dashboard.py \
   /input/platforms/raspberrypi/car-diag.sh \
   /input/platforms/raspberrypi/carfluxoid-dashboard.service \
   /input/platforms/raspberrypi/carfluxoid-car-diag.service \
   /input/platforms/raspberrypi/install-dashboard.sh \
   /input/tooling/waiting_iphone_config.py \
   "$bundle/"

(
 cd "$bundle"
 sha256sum aa-phone catplay-aa-rx start-radio.sh start-ap-trial.sh \
  dashboard.py car-diag.sh carfluxoid-dashboard.service \
  carfluxoid-car-diag.service waiting_iphone_config.py > SHA256SUMS
 sha256sum -c SHA256SUMS
)
tar -czf "$out/carfluxoid-update.tar.gz" -C "$out" update-bundle
sha256sum "$out/aa-phone" "$out/catplay-aa-rx" "$out/carfluxoid-update.tar.gz" > "$out/SHA256SUMS"
ls -lh "$out/aa-phone" "$out/catplay-aa-rx" "$out/carfluxoid-update.tar.gz"
