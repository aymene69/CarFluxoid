# Reproducibility status

The previous source-only export has been replaced by the build/install/recovery chain in [INSTALL.md](INSTALL.md).

Included now: pinned source downloader, clean Docker SDK bootstrap, bridge/module compilation, all display assets, deployment assembly, verified EROFS packaging, per-device composition, SSH backup/flash/readback, native FEL backup/flash/full restore, and offline tests/CI.

Still intentionally external:

- The owner's dongle and its original/private identity data. The commands back these up locally; they are never copied from another owner or uploaded to GitHub.
- A compatible head unit or Google DHU for hardware acceptance. Google DHU is not redistributed.
- Host USB access and native libusb/compiler dependencies.
- The pinned public CatPlay beta downloaded during the build. It supplies the known kernel, vendor init flow and radio blobs; this project does not claim to rebuild those proprietary components from source.

The initial factory-to-beta flashing path is implemented and guarded by SoC/flash checks, double backup and complete readback, but has not yet been repeated on a second freshly purchased device. The radio revision must be confirmed separately. The commercial product name is insufficient because boards are swapped under the same name.

See [validation evidence](VALIDATION.md) for the clean-build tests, known reference hashes and hardware limits.
