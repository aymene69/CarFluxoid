# Build environment

The current cross-build scripts are exported from the working development environment. They assume:

- Linux, Clang/LLVM/LLD 19 and Rust with `rust-src` / build-std support.
- RISC-V RV32GC, ilp32d, musl target and matching firmware runtime libraries.
- A musl 1.2.5 CRT, MbedTLS build and OpenSSL static build under `/build/aa`.
- D-Bus and Opus runtime libraries from the firmware, supplied locally under `/input/work/uconnect/dongle-root`.
- This repository mounted read-only at `/input`, with build output under `/build`.
- The exact beta kernel headers/build tree for compiling `aa_bulk.ko`.

After preparing CatPlay with `scripts/prepare-catplay.sh`:

```sh
sh /input/bridge-aa/build-v821.sh
sh /input/bridge-aa/build-carplay-rx.sh
```

These scripts do not download or bootstrap the SDK. Do not substitute a rebuilt kernel or arbitrary kernel module for the beta versions without validation. The working rootfs uses EROFS LZ4 with **4096-byte blocks**; an image built with 16 KiB blocks failed to boot on the device.

## Tests used during development

C tests include `parser.c`, `microphone.c` and `video_config.c`. They include the production sender and link the host MbedTLS libraries. On a 64-bit Linux host, the legacy unused AIO branch needs the compile definition `SYS_io_pgetevents_time64=SYS_io_pgetevents`.

Rust unit tests are built for RV32 using build-std and run with QEMU and the firmware libc. This matters: native host tests did not reveal an RV32 metadata ABI crash. The settings HTTP test accepts `AA_WEB_TEST_COMMAND` to launch the real cross-built executable under QEMU, for example:

```sh
AA_WEB_TEST_COMMAND='qemu-riscv32 -L /input/work/uconnect/dongle-root /build/aa/carfluxoid-rx-microphone --web' \
  python3 /input/bridge-aa/tests/test_web.py
```

The HTTP test binds port 80 in the isolated Linux build environment. It verifies consecutive writes, validation, server restart and persistence. Device certificates/private keys are unnecessary for these tests.

## Deployment templates

`deploy/` is not a flash installer. It expects previously prepared binaries, TLS test credentials, device identity, images and an initialized beta boot environment. Use `deploy/wifi-env.sh.example` as the credential-free starting point for `wifi-env.sh`; actual credentials belong in local configuration. The required test pattern is included as `assets/mire.h264`, and `deploy/catplay_c2a` is the startup wrapper. The existing receiver reads the actual firmware-generated Wi-Fi configuration before fallback values.

Keep device-specific image building and backup/restore tooling outside Git until a reproducible, reviewed packaging flow is available.
