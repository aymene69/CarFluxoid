# Repository completeness audit — 14 November 2026

A fresh clone and recursive CatPlay checkout were tested. `scripts/prepare-catplay.sh` applied the tracked patch successfully to commit `471b0114ad2344cca9cb1d775e318a6ab175547a`.

## Included

- Bridge C/Rust source, kernel gadget source, Cargo manifests and lockfiles.
- Pinned upstream CatPlay dependency and the local patch, including its added test.
- Original waiting PNGs and encoded H264 assets; conversion script.
- Test pattern `assets/mire.h264` required by the autonomous command line.
- Bootstrap, session and web supervisor templates; `deploy/catplay_c2a` entrypoint.
- Credential-free `deploy/wifi-env.sh.example` fallback configuration.
- Protocol, display selection, microphone and settings tests.

## Still external or missing for a standalone firmware build

1. SDK bootstrap: build scripts assume the prepared `/build/aa` Linux cross-toolchain, musl CRT, MbedTLS, OpenSSL and firmware runtime sysroot. A reproducible SDK Dockerfile/bootstrap is not supplied yet.
2. Exact beta kernel build tree/configuration for `aa_bulk.ko`; the repository contains gadget source, not a ready module or the kernel SDK.
3. Device-specific original firmware and identity. SPI backups, MFi certificate/private key, pairing data and passwords must be supplied locally and must not be committed.
4. Android Auto TLS test credentials used by the sender must be supplied locally. The repository does not include private keys and does not promise that an arbitrary self-signed pair will be accepted by every receiver.
5. Rootfs assembler, partition verifier, flash installer and verified recovery tooling. The working workspace has these tools, but they have not yet been made portable and exported here. No standalone flash instructions are provided.
6. Automated CI has not been configured. Tests documented here were run in the development environment.

## Autonomous rootfs payload contract

`/usr/lib/aa-bridge/` must contain:

- `catplay-aa-rx.gz`, `aa-phone-v821.gz`, `aa_bulk.ko.gz` from compatible builds.
- `boot.sh`, `usb-test.sh`, `web-service.sh` from `deploy/`.
- `wifi-env.sh` derived from the example, keeping actual credentials local.
- `mire.h264` from `assets/`.
- The three `attente-*.h264` files from `assets/waiting/`.
- Locally supplied `test-cert.pem` and `test-key.pem`.

`deploy/catplay_c2a` is the `/usr/bin/catplay_c2a` wrapper. It assumes the original beta init flow and firmware radio setup. The device's own identity remains under `/persist`. The exact firmware must be inspected and backed up before packaging; copying this directory is not a substitute for a validated image builder.

A successful source checkout therefore does **not** mean a third party can compile and flash the complete working firmware without preparing those external components.
