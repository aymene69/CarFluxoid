# Raspberry Pi Zero 2 W Target

This directory contains the complete ARM64 build, systemd service topology, projected touchscreen dashboard, and installers for the **Raspberry Pi Zero 2 W**.

- Full setup instructions: [docs/INSTALL.md](../../docs/INSTALL.md)
- System architecture & IPC layout: [docs/ARCHITECTURE.md](../../docs/ARCHITECTURE.md)
- Projected touchscreen menu details: [dashboard/README.md](dashboard/README.md)

## Quick Commands (from repository root)

```sh
make prepare                       # init catplay submodule & apply patches/catplay.patch
make image                         # build ARM64 Docker image (platforms/raspberrypi/Dockerfile)
make build                         # build binaries + local/raspberrypi/carfluxoid-update.tar.gz
make test                          # run full Rust, C, HTTP, MTP, and dashboard test suites
make deploy HOST=aymene@192.168.50.2
make logs   HOST=aymene@192.168.50.2
```

## Installed Systemd Services

| Service | Role |
| :--- | :--- |
| `carfluxoid-aa.service` | Loads `dwc2` + `aa_bulk` (`aoa_protocol=2`) and runs `aa-phone` with `RuntimeDirectoryPreserve=yes` and `RestartSec=0.2` |
| `carfluxoid-dashboard.service` | Renders the projected touchscreen menu and triggers auto-start after a 2 s settle delay |
| `carfluxoid-radio.service` | Runs `catplay-aa-rx` (wireless CarPlay receiver, 4 Tokio workers, iAP2 GPS/NowPlaying hub) |
| `carfluxoid-web.service` | Runs `catplay-aa-rx --web` on port 80 (`http://192.168.50.2`) for settings, diagnostics, logs, and OTA updates |
| `carfluxoid-car-diag.service` | Persists bounded vehicle diagnostic snapshots to `/var/lib/carfluxoid/car-diag.log` |
