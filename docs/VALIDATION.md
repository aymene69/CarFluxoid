# Validation record — 14 November 2026

## Performed in a clean build environment

- A new SDK Docker image was built from the pinned base image and Dockerfile.
- A new, empty `carfluxoid-sdk-clean-off` volume was populated using only the tracked scripts and checked source archives. The old development SDK's compiled libraries were not copied in.
- The public beta **hstimer-off** and Linux archives were additionally downloaded afresh and their checksums matched the lockfile.
- The bridge C executable, Rust receiver, USB module and EROFS partition were rebuilt successfully.
- The module executable section matched the hardware-tested reference exactly: `359388ed97e1608a1796c65694086139bd55fd4e1ea2f84d887f70f117e2967b`.
- The rootfs was re-extracted and compared against the staged tree; 4 KiB blocks and partition fit were checked.
- An image was composed against a real local supported backup. Boot/kernel/persist preservation was verified without publishing that backup or its identifiers.
- Offline tests cover image composition, rejected layouts/kernel changes, two-read backups, simulated SPI writing and detection of a readback mismatch.
- The rebuilt RV32 AA sender was connected over TCP/QEMU to Google's unchanged DHU 2.0-mac, build `2022-03-30-438482292`, in headless mode. TLS 1.2 verification returned OK and **27 video frames were sent and acknowledged** during a 15-second test. This also validated the pinned public reference TLS pair used by the new build.

## Previously exercised on the development dongle

Autonomous boot, iPhone CarPlay video, touch and music worked with Google DHU on the supported V821 non-B / RTL8733BS device. The rootfs-only FEL repair path with full readback was used successfully during development. Later scale/microphone/display options require their own hardware checks.

## Not yet claimed

- A new factory device has not been purchased and run through every new CLI command end-to-end.
- The new CLI's physical write commands were tested with an in-memory SPI model, not executed against the owner's currently working dongle during this export.
- Factory radio variant detection cannot be inferred solely from FEL SoC ID and SPI capacity.
- Uconnect compatibility and microphone/Siri are not established by the TCP DHU test.
- No promise is made that arbitrary devices sold under the same marketing name or all head units are compatible.

The vehicle-startup/AOA candidate changes the gadget module and supervisors.
Its hardware and car validation are pending; see [VEHICLE-TEST.md](VEHICLE-TEST.md).
