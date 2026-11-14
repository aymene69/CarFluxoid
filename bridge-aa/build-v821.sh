#!/bin/sh
# Run inside the prepared Linux container, after building musl CRT and MbedTLS.
set -eu
build=/build/aa
clang-19 --target=riscv32-linux-musl -march=rv32gc -mabi=ilp32d \
 --sysroot="$build/sysroot" -isystem "$build/sysroot/include" \
 -I"$build/mbedtls-rv/include" -Os -ffunction-sections -fdata-sections \
 -fno-stack-protector -c /input/bridge-aa/src/aa_phone.c -o "$build/aa-phone-rv.o"
ld.lld-19 -m elf32lriscv --gc-sections --dynamic-linker=/lib/ld-musl-riscv32.so.1 \
 "$build/musl-1.2.5/obj/crt/crt1.o" "$build/musl-1.2.5/obj/crt/crti.o" \
 "$build/aa-phone-rv.o" "$build/mbedtls-rv/library/libmbedtls.a" \
 "$build/mbedtls-rv/library/libmbedx509.a" "$build/mbedtls-rv/library/libmbedcrypto.a" \
 /input/work/uconnect/dongle-root/usr/lib/libc.so \
 /input/work/uconnect/dongle-root/lib/libgcc_s.so.1 \
 "$build/musl-1.2.5/obj/crt/crtn.o" -o "$build/aa-phone-v821"
