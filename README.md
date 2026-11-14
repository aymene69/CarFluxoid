<p align="center">
  <img src="assets/carfluxoid-logo.png" alt="CarFluxoid" width="420">
</p>

# CarFluxoid

Experimental bridge: **iPhone wireless CarPlay → Allwinner V821 dongle → wired Android Auto head unit**.

The dongle receives CarPlay itself. A native Android Auto sender forwards H.264 video and PCM audio and translates touchscreen input back to the iPhone. Google Desktop Head Unit is used for development.

## Current status

Video, touch and music have worked with Google DHU on a V821 non-B dongle using the CatPlay issue 56 beta kernel. The latest session lifecycle version was reported working on 14 November 2026. Uconnect compatibility remains unverified. LIVI is not a validated receiver for this bridge.

The code also includes a microphone return path, persistent pairing, a local settings page, an OEM return button and a waiting screen mechanism. Microphone/Siri, recent display options and repeated disconnect/reconnect behavior still need hardware validation. This is a development project, not a general-purpose firmware release.

## Source layout

- `bridge-aa/src/aa_phone.c`: native Android Auto sender and video/audio/input transport.
- `bridge-aa/kernel/aa_bulk.c`: experimental USB gadget with prequeued RX requests for the beta kernel.
- `bridge-aa/carplay-rx`: Rust CarPlay receiver, audio conversion, microphone and HTTP settings.
- `bridge-aa/tests`: protocol, settings and transport tests.
- `catplay`: upstream CatPlay submodule pinned to the tested source revision.
- `patches/catplay.patch`: local CatPlay changes, including the RV32 filesystem workaround and accessory identity.
- `deploy`: bootstrap files automatically assembled into the rootfs.
- `carfluxoid.py`: source fetch, build/test, device backup, compose, flash and recovery CLI.
- `tooling`: source lock, Docker SDK bootstrap, packaging and device verification.
- `assets/waiting`: the three CarFluxoid waiting screens, in PNG and ready-to-use H.264.

```sh
git clone --recurse-submodules https://github.com/aymene69/CarFluxoid.git
cd CarFluxoid
./scripts/prepare-catplay.sh
```

Start with the complete [build, backup, install and recovery guide](docs/INSTALL.md). See also [build internals](docs/BUILD.md), [display settings](docs/DISPLAY.md) and [validation evidence](docs/VALIDATION.md).

Firmware images, SPI backups, device identity certificates, keys, pairing databases, passwords and diagnostic captures are deliberately excluded. They must remain local. The repository now bootstraps its own Docker SDK and provides per-device image composition, verified flashing and recovery commands for the supported hardware profile. A full fresh-factory-device acceptance test is still pending; see the validation record.

## License and attribution

The bridge includes its existing GPL-3.0 license in `LICENSE`. Upstream dependencies and imported components retain their own licenses and notices. CatPlay is maintained at https://github.com/catplay-labs/catplay; local modifications are tracked separately rather than rewriting its upstream history.

## Waiting screens

Original artwork and matching H.264 files are in [`assets/waiting`](assets/waiting), for 800×480, 1280×720 and 1920×1080. The autonomous bootstrap expects these H.264 files beside the bridge payload. To regenerate them from the PNGs, install Pillow and FFmpeg, then run `python3 scripts/encode-waiting-screens.py`.
