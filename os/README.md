# CarFluxoidOS — Custom Embedded Linux for Raspberry Pi Zero 2 W

**CarFluxoidOS** is a minimal, purpose-built Buildroot ARM64 Linux distribution designed exclusively for turning a **Raspberry Pi Zero 2 W** into a fast-booting, power-cut-proof Wireless CarPlay → Wired Android Auto appliance.

---

## Key Design Goals

- **Fast boot (~4 s USB / ~5 s Wi-Fi)**: `S15usb` loads `dwc2` + `aa_bulk.ko` and starts `aa-phone` immediately — no `systemd`, no `NetworkManager`, no `apt`.
- **Power-cut proof**: immutable **ZSTD SquashFS** root filesystem (`ro`). Cutting vehicle ignition abruptly cannot corrupt the OS.
- **Atomic A/B OS Updates (Zero Data Loss)**: dual rootfs slots (`mmcblk0p2` Slot A and `mmcblk0p3` Slot B) with a separate persistent `/data` partition (`mmcblk0p4`, ext4 with automatic `e2fsck -p` at boot). Running `make os-update` (or streaming `carfluxoid-os-update.tar` into `/usr/sbin/carfluxoid-os-update`) flashes and SHA256-verifies the inactive slot on microSD before atomically switching `cmdline.txt` — **never touching `/data`** (iAP2 certificates, HomeKit/Bluetooth pairings, Wi-Fi passphrase, SSH host keys, and Web UI settings stay intact forever).
- **Smooth CarPlay performance**:
  - `performance` CPU governor (1000 MHz) + preloaded ARM64 NEON crypto modules (`chacha-neon`, `poly1305-neon`, `aes-neon-blk`, `aes-neon-bs`) for hardware-accelerated AirPlay decryption.
  - `nice -n -5` scheduling priority for `aa-phone` and `catplay-aa-rx`.
  - 4 MiB kernel socket buffers (`/etc/sysctl.conf`), adaptive BCM43430 Wi-Fi TX power (`txpower auto`), `power_save off`, and 802.11n `SHORT-GI-20`.
  - HDMI blanked, onboard audio/camera disabled, PL011 UART0 reserved exclusively for Bluetooth (`enable_uart=0`), `dtoverlay=dwc2,dr_mode=peripheral`.
- **Out-of-tree `aa_bulk.ko` compiled in lockstep with the kernel**: zero manual kernel headers or `make modules` steps on the Pi.
- **Drop-in credential provisioning**: copy `certificate.p7b` and `identity.pk8` (and optional `wifi-pass.txt` / `authorized_keys`) onto the FAT32 `CARFLUXBOOT` partition right after flashing; on first boot `S05data` moves them to `/data/identity/` (`0600`) and deletes the staging files from `/boot`.

---

## Partition Layout (`carfluxoid-os.img`)

| Partition | Label | Filesystem | Size | Mount / Role |
| :---: | :--- | :--- | :---: | :--- |
| `mmcblk0p1` | `CARFLUXBOOT` | FAT32 | 64 MiB | Pi bootloader, `Image`, DTB, `overlays/dwc2.dtbo`, `config.txt`, `cmdline.txt` |
| `mmcblk0p2` | — | SquashFS (ZSTD) | 128 MiB | Immutable read-only **Slot A** rootfs |
| `mmcblk0p3` | — | SquashFS (ZSTD) | 128 MiB | Immutable read-only **Slot B** rootfs (atomic A/B update target) |
| `mmcblk0p4` | `CARFLUXDATA` | ext4 | 256 MiB | Persistent `/data` (`/var/lib/carfluxoid`, `/var/lib/bluetooth`, `/etc/dropbear`, OverlayFS `/opt/carfluxoid`) |

---

## Build from Source

From the repository root:

```sh
make build      # builds ARM64 aa-phone + catplay-aa-rx
make tls        # fetches verified reference Android Auto TLS pair into local/tls/
make os-image   # builds the Buildroot Docker image (once)
make os         # compiles CarFluxoidOS -> local/os/carfluxoid-os.img.xz + carfluxoid-os-update.tar + SHA256SUMS
```

---

## Flash & First Boot (Raspberry Pi Imager)

1. Flash `carfluxoid-os.img.xz` onto a microSD card using **Raspberry Pi Imager** (*Use custom*, click **No** for OS customisation).
2. Re-mount the `CARFLUXBOOT` FAT32 partition on your Mac/PC and copy your iAP2 / MFi authentication certificates onto its root (for example from [Roylyl/MacPlay — `assets/authentication`](https://github.com/Roylyl/MacPlay/tree/main/assets/authentication)):
   - `certificate.p7b`
   - `identity.pk8`
   - *(optional)* `wifi-pass.txt` (8–63 chars) to override the default `CarFluxoid-Pi_AP` passphrase (`c@rfluxoid`)
   - *(optional)* `authorized_keys` for passwordless SSH (`root@192.168.50.2`, default password `c@rfluxoid`)
3. Eject the SD card, insert it into the Raspberry Pi Zero 2 W, and plug a short micro-USB **data** cable from your car’s Android Auto USB port into the Pi’s **USB** port (closest to mini-HDMI).
4. Pair your iPhone with Bluetooth **`CarFluxoid`** and accept CarPlay.
5. Configure settings, view live diagnostics/logs, or upload app OTA bundles at `http://192.168.50.2` while joined to Wi-Fi **`CarFluxoid-Pi_AP`** (password `c@rfluxoid`).

---

## Updates & Backups (Preserves `/data`)

```sh
# Fast app-only update via OverlayFS (no reboot, keeps /data)
make deploy

# Full atomic A/B OS update over SSH (kernel + DTB + rootfs, keeps /data)
make os-update
# or directly with a downloaded release archive:
ssh root@192.168.50.2 /usr/sbin/carfluxoid-os-update < carfluxoid-os-update.tar

# Backup & restore /data (certificates, pairings, settings, SSH keys)
make backup
make restore
```
