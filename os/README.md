# CarFluxoidOS — Custom Embedded Linux for Raspberry Pi Zero 2 W

**CarFluxoidOS** is a minimal, purpose-built Buildroot ARM64 Linux distribution designed exclusively for turning a **Raspberry Pi Zero 2 W** into a fast-booting, power-cut-proof Wireless CarPlay → Wired Android Auto appliance.

---

## Key Design Goals

- **Fast boot (~5 s to USB Android Auto)**: `S15usb` loads `dwc2` + `aa_bulk.ko` and starts `aa-phone` before Wi-Fi, Bluetooth, and the dashboard finish initializing — no `systemd`, no `NetworkManager`, no `apt`.
- **Power-cut proof**: immutable **ZSTD SquashFS** root filesystem (`ro`). Cutting vehicle ignition abruptly cannot corrupt the OS.
- **Writable `/data` + OverlayFS `/opt/carfluxoid`**: settings, HomeKit pairings, BlueZ Bluetooth pairings, MFi identity, diagnostic logs, and SSH host keys live on partition 3 (`CARFLUXDATA`, ext4 with automatic `e2fsck -p` at boot). `/opt/carfluxoid` is mounted via OverlayFS so `make deploy` and Web OTA (`http://192.168.50.2`) still update binaries in seconds without reflashing the SD card.
- **Single-USB low-power profile**:
  - `config.txt`: `arm_freq=750`, `gpu_mem=16`, HDMI blanked, onboard audio/camera/LED disabled, PL011 UART0 reserved exclusively for Bluetooth (`enable_uart=0`), `dtoverlay=dwc2,dr_mode=peripheral`.
  - `S30wifi`: `iw dev wlan0 set txpower fixed 1000` (10 dBm — plenty for inside a car cabin, saves ~100 mA TX spikes) + `power_save off` for low RTP audio/video jitter.
- **Out-of-tree `aa_bulk.ko` compiled in lockstep with the kernel**: zero manual kernel header or `make modules` steps on the Pi.
- **Drop-in credential provisioning**: drop `certificate.p7b` and `identity.pk8` (and optional `authorized_keys`) onto the FAT32 `CARFLUXBOOT` partition right after flashing; on first boot `S05data` moves them to `/data/identity/` (`0600`) and deletes the staging files from `/boot`.

---

## Partition Layout (`carfluxoid-os.img`)

| Partition | Label | Filesystem | Size | Mount / Role |
| :---: | :--- | :--- | :---: | :--- |
| `mmcblk0p1` | `CARFLUXBOOT` | FAT32 | 64 MiB | Pi bootloader, `Image`, DTB, `overlays/dwc2.dtbo`, `config.txt`, `cmdline.txt` |
| `mmcblk0p2` | — | SquashFS (ZSTD) | 160 MiB | Immutable read-only rootfs (`/`) |
| `mmcblk0p3` | `CARFLUXDATA` | ext4 | 256 MiB | Persistent `/data` (`/var/lib/carfluxoid`, `/var/lib/bluetooth`, OverlayFS `/opt/carfluxoid`) |

---

## Build from Source

From the repository root:

```sh
make build      # builds ARM64 aa-phone + catplay-aa-rx
make tls        # fetches verified reference Android Auto TLS pair into local/tls/
make os-image   # builds the Buildroot Docker image (once)
make os         # compiles CarFluxoidOS -> local/os/carfluxoid-os.img.xz + SHA256SUMS
```

---

## Flash & First Boot

1. Flash `local/os/carfluxoid-os.img.xz` onto a microSD card using **Raspberry Pi Imager** (*Use custom*) or `dd`.
2. Re-mount the `CARFLUXBOOT` FAT32 partition on your Mac/PC and copy your private MFi files onto its root:
   - `certificate.p7b`
   - `identity.pk8`
   - *(optional)* `wifi-pass.txt` (8–63 chars) to set your own `CarFluxoid-Pi_AP` Wi-Fi passphrase (otherwise a random 24-hex passphrase is generated in `/data/radio.env`)
   - *(optional)* `authorized_keys` for passwordless SSH (`root@192.168.50.2`, default password `c@rfluxoid`, same for `CarFluxoid-Pi_AP` Wi-Fi)
3. Eject the SD card, insert it into the Raspberry Pi Zero 2 W, and plug a short micro-USB **data** cable from your car’s Android Auto USB port into the Pi’s **USB** port (closest to mini-HDMI).
4. Pair your iPhone with Bluetooth **CarFluxoid** and accept CarPlay.
5. Configure settings, view live diagnostics/logs, or upload OTA bundles at `http://192.168.50.2` while joined to Wi-Fi **`CarFluxoid-Pi_AP`**.
