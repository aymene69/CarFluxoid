#!/bin/sh
# Run the full CarFluxoid verification suite inside the ARM64 build container.
set -eu

host=/build/pi/mbedtls
out=/input/local/raspberrypi
export CARGO_TARGET_DIR=/build/pi/target
export OPENSSL_NO_VENDOR=1

echo "=== Rust unit tests ==="
cargo test --locked --manifest-path /input/bridge-aa/carplay-rx/Cargo.toml

echo "=== C protocol & unit tests ==="
cd /input/bridge-aa
for t in parser video_config audio_flow microphone car_controls sensors mtp aoa reader_eof; do
 echo "--- C test: $t ---"
 cc -O2 -DSYS_io_pgetevents_time64=SYS_io_pgetevents \
  -Isrc -Ikernel -I"$host/include" "tests/$t.c" \
  "$host/library/libmbedtls.a" "$host/library/libmbedx509.a" "$host/library/libmbedcrypto.a" \
  -lpthread -o "/tmp/test_$t"
 "/tmp/test_$t"
done

echo "=== Runtime HTTP & MTP tests ==="
AA_WEB_TEST_COMMAND="$out/catplay-aa-rx --web" python3 /input/bridge-aa/tests/test_web.py
AA_MTP_TEST_COMMAND="$out/aa-phone" python3 /input/bridge-aa/tests/test_mtp_runtime.py

echo "=== Python dashboard & syntax checks ==="
cd /input
python3 -m unittest discover -s tests -v
python3 -m py_compile platforms/raspberrypi/dashboard/dashboard.py tooling/waiting_iphone_config.py scripts/fetch-tls.py scripts/encode-waiting-screens.py
for script in scripts/*.sh platforms/raspberrypi/*.sh; do
 sh -n "$script"
done

echo "=== ALL TESTS PASSED ==="
