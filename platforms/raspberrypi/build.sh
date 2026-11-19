#!/bin/sh
# Native ARM64 Linux container; independent from the V821 output directory.
set -eu
[ "$(uname -m)" = aarch64 ] || { echo 'ARM64 Linux build environment required'; exit 1; }
out=/input/local/raspberrypi
mkdir -p "$out" /build/pi
host=/build/pi/mbedtls
if [ ! -f "$host/Makefile" ];then
 python3 - <<'PREPARE'
from pathlib import Path
import hashlib,json,tarfile,urllib.request
spec=json.loads(Path('/input/tooling/sources.lock.json').read_text())['sources']['mbedtls']
cache=Path('/build/pi/mbedtls.tar.gz')
if not cache.exists() or hashlib.sha256(cache.read_bytes()).hexdigest()!=spec['sha256']:
 with urllib.request.urlopen(spec['url'],timeout=120) as response:cache.write_bytes(response.read())
assert hashlib.sha256(cache.read_bytes()).hexdigest()==spec['sha256']
dest=Path('/build/pi/mbedtls-extract');dest.mkdir(exist_ok=True)
with tarfile.open(cache) as archive:archive.extractall(dest,filter='data')
next(dest.iterdir()).rename('/build/pi/mbedtls')
PREPARE
fi
make -C "$host" -j4 lib
cc -O2 -DSYS_io_pgetevents_time64=SYS_io_pgetevents -I"$host/include" /input/bridge-aa/src/aa_phone.c \
 "$host/library/libmbedtls.a" "$host/library/libmbedx509.a" "$host/library/libmbedcrypto.a" -lpthread -o "$out/aa-phone"
export CARGO_TARGET_DIR=/build/pi/target OPENSSL_NO_VENDOR=1
cargo build --release --locked --manifest-path /input/bridge-aa/carplay-rx/Cargo.toml
cp "$CARGO_TARGET_DIR/release/catplay-aa-rx" "$out/catplay-aa-rx"
strip "$out/aa-phone" "$out/catplay-aa-rx"
"$out/catplay-aa-rx" --self-test
sha256sum "$out/aa-phone" "$out/catplay-aa-rx" > "$out/SHA256SUMS"
