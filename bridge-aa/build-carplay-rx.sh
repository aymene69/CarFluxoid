#!/bin/sh
# Run in the existing Linux build container. Build std for the tier-3 RV32 target.
set -eu
build=/build/aa
runtime=${CARFLUXOID_SYSROOT:-/build/firmware-root}
mkdir -p "$build/bin" "$build/pkgconfig" "$build/sysroot/lib"
cat > "$build/bin/rv32-cc" <<'EOF'
#!/bin/sh
exec clang-19 --target=riscv32-linux-musl -march=rv32gc -mabi=ilp32d --sysroot=/build/aa/sysroot -isystem /build/aa/sysroot/include "$@"
EOF
chmod +x "$build/bin/rv32-cc"
for lib in dbus-1 opus; do
case "$lib" in dbus-1) soname=libdbus-1.so.3.38.3; version=1.14.10;; opus) soname=libopus.so.0.11.1; version=1.3.1;; esac
ln -sf "${runtime}/usr/lib/$soname" "$build/sysroot/lib/lib$lib.so"
cat > "$build/pkgconfig/$lib.pc" <<EOF
Name: $lib
Description: Dongle runtime library
Version: $version
Libs: -L$build/sysroot/lib -l$lib
Cflags:
EOF
done
export CARGO_HOME=${CARGO_HOME:-/build/cargo} RUSTUP_HOME=${RUSTUP_HOME:-/build/rustup}
export CARGO_TARGET_DIR=/build/aa/cp-rx-target
export RUSTC_BOOTSTRAP=1
export CC_riscv32gc_unknown_linux_musl="$build/bin/rv32-cc"
export CXX_riscv32gc_unknown_linux_musl="$build/bin/rv32-cc"
export CXXSTDLIB_riscv32gc_unknown_linux_musl=''
export AR_riscv32gc_unknown_linux_musl=llvm-ar-19
export OPENSSL_NO_VENDOR=1 OPENSSL_DIR=/build/aa/openssl-install OPENSSL_STATIC=1
export PKG_CONFIG_ALLOW_CROSS=1 PKG_CONFIG_LIBDIR="$build/pkgconfig"
for name in c m pthread dl rt util resolv; do
ln -sf ${runtime}/usr/lib/libc.so "$build/sysroot/lib/lib$name.so"
done
ln -sf ${runtime}/lib/libgcc_s.so.1 "$build/sysroot/lib/libgcc_s.so"
cat > "$build/bin/rv32-link" <<'EOF'
#!/usr/bin/env python3
import os,sys
runtime=os.environ.get('CARFLUXOID_SYSROOT','/build/firmware-root')
# These build scripts unconditionally request runtime archives even when none
# of their symbols are used. The final linker still rejects unresolved symbols.
args=[a for a in sys.argv[1:] if a not in ('-lstdc++','-latomic')]
os.execvp('clang-19',['clang-19','--target=riscv32-linux-musl','-march=rv32gc','-mabi=ilp32d','-fuse-ld=lld','-nostdlib','/build/aa/musl-1.2.5/obj/crt/crt1.o','/build/aa/musl-1.2.5/obj/crt/crti.o','-L/build/aa/sysroot/lib','-Wl,--dynamic-linker=/lib/ld-musl-riscv32.so.1',*args,'-lgcc_s','/build/aa/musl-1.2.5/obj/crt/crtn.o'])
EOF
chmod +x "$build/bin/rv32-link"
export CARGO_TARGET_RISCV32GC_UNKNOWN_LINUX_MUSL_LINKER="$build/bin/rv32-link"
export CARGO_TARGET_RISCV32GC_UNKNOWN_LINUX_MUSL_RUSTFLAGS='-C target-feature=-crt-static -C panic=abort -C link-self-contained=no'
exec "$CARGO_HOME/bin/cargo" build --release --locked -Zbuild-std=std,panic_abort --target riscv32gc-unknown-linux-musl --manifest-path /input/bridge-aa/carplay-rx/Cargo.toml
