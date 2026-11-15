# Reproducible build

The supported entry point is documented in [INSTALL.md](INSTALL.md). It does not require the author's old Docker container or workstation directories.

```sh
.venv/bin/python carfluxoid.py fetch
.venv/bin/python carfluxoid.py sdk
.venv/bin/python carfluxoid.py build
.venv/bin/python carfluxoid.py test
```

## Locked inputs

`tooling/sources.lock.json` records source archive URLs/SHA-256, Rust 1.100.0, the Docker base image digest, the **issue 56 hstimer-off** beta image/kernel hash, the module configuration and reference exports. Cargo dependencies are locked separately. Every cached source is rechecked before use.

The clean build constructs musl 1.2.5 headers/CRT, MbedTLS 2.28.10, OpenSSL 3.6.3, the native AA sender, the Rust CarPlay receiver and `aa_bulk.ko`. The SDK uses Clang/LLVM/LLD 19. The kernel header preparation uses pinned Linux 7.2.13 and the needed vendor public-header/RISC-V patches. The beta kernel itself is not replaced by a fresh kernel build.

`configs/beta-exports.symvers` is the reference kernel's generated export-name table, not a device identity. `CONFIG_MODVERSIONS` is disabled. The compiled gadget's executable section must match the hardware-tested reference hash before packaging, in addition to checking its architecture and kernel build inputs. This caught/guards kernel-profile drift without requiring an untested replacement kernel.

## Outputs and local state

- `local/downloads/`: checked source downloads.
- Docker volume `carfluxoid-sdk`: generated SDK, source trees and compiler cache.
- `local/base/`: checked, unpacked public beta and its reference manifest.
- `local/tls/`: local reference or user-supplied AA TLS pair.
- `local/artifacts/`: cross-built executables/module, EROFS root and manifest.
- Per-device backup and composed images: wherever explicitly requested under `local/`.

The image builder retains all original beta files except `/usr/bin/catplay_c2a`, then adds `/usr/lib/aa-bridge`. It verifies EROFS **4096-byte blocks**, the 11 MiB partition limit, a full extraction/tree comparison and RV32 ELF architecture. Composition verifies the actual backup's supported kernel and preserves its boot/kernel/persist. Flash takes fresh backups and rebases persist onto the latest device bytes before writing.

Do not commit `local/`, identity files or composed device images. They are ignored. The project does not include someone's MFi private key, pairing database or SPI backup.

## Checks

`test` runs native C protocol/selection/EOF tests, the production RV32 settings executable under QEMU and the firmware libc, RV32 recorder/scale/socket tests, and simulated image/flash/readback tests. GitHub source checks run automatically; a separate manually dispatched workflow runs the full SDK/build/test chain without touching hardware.

A successful offline build or simulated flash is not a hardware acceptance test. Consult [VALIDATION.md](VALIDATION.md) for what has actually been exercised.

### Production logging

Autonomous firmware uses production logging by default. Android Auto packet,
TLS-counter, per-audio-block and per-video-frame traces are disabled, as are
`usb-rx.bin` and `aa-rx.bin` captures. CarPlay keeps warnings and errors; Android
Auto keeps connection transitions and errors. Waiting videos are symlinked from
the read-only firmware rather than copied into RAM.

For a manual diagnostic session, export `AA_DIAGNOSTIC=1` before starting the
bridge; `RUST_LOG=info` (or `debug`) can independently override Rust logging.
Diagnostic captures can grow without limit and should only be used temporarily.
The autonomous log maintenance process checks the four service logs every ten
seconds and truncates files exceeding 64 KiB in place. This is a periodic limit,
not a strict byte ceiling: output between checks can exceed the threshold.
The web server and all radio, audio, microphone and touch services remain active.

### Autonomous UTC clock

At bridge startup, `clock-service.sh --restore` restores `/persist/aa-clock-epoch`
only if it is newer than the current clock. This is a lower bound, not an RTC:
powered-off time cannot be reconstructed from that file. The receiver advertises
`DeviceTimeUpdate` reception over iAP2. With `AA_CLOCK_SYNC=1`, valid iPhone UTC
seconds (2020–2099) are passed to the clock service, applied within two seconds
and atomically saved to persist. Time-zone offsets are not added to UTC.
This requires the connected iPhone to send that message; real-device receipt
must be verified using `CLOCK_SYNCED` in the boot log. Before a phone connects,
only the saved lower bound is available. Transport/media timers stay monotonic.
