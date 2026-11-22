# Installation Guide — Raspberry Pi Zero 2 W

This guide covers a clean, from-scratch installation of CarFluxoid on a **Raspberry Pi Zero 2 W** running **Raspberry Pi OS Lite (64-bit)**.

---

## 1. Prepare the SD Card

1. Flash **Raspberry Pi OS Lite (64-bit)** using Raspberry Pi Imager.
2. Enable **SSH** and configure your 2.4 GHz home Wi-Fi so you can reach the Pi during initial setup.
3. Boot the Pi and install kernel headers + build tools for compiling `aa_bulk.ko`:
   ```sh
   sudo apt-get update
   sudo apt-get install -y --no-install-recommends linux-headers-rpi-v8 gcc make
   ```

---

## 2. Build the ARM64 Binaries (Host Machine)

From the repository root on your Mac or Linux machine:

```sh
make prepare
make image
make build
make test
make tls
```

Outputs in `local/raspberrypi/` (all gitignored):
- `aa-phone` (Android Auto transport)
- `catplay-aa-rx` (Wireless CarPlay receiver + Web/OTA server)
- `update-bundle/` and `carfluxoid-update.tar.gz` (verified update bundle)
- `local/tls/test-cert.pem` and `local/tls/test-key.pem` (reference AA TLS pair)

---

## 3. Build `aa_bulk.ko` on the Pi

`aa_bulk.ko` must match the Pi’s running kernel. Copy the kernel module source to the Pi and build it locally:

```sh
ssh pi@<PI_IP> 'mkdir -p ~/carfluxoid-build/kernel ~/carfluxoid-build/assets'
scp bridge-aa/kernel/aa_bulk.c bridge-aa/kernel/aa_aoa.h bridge-aa/kernel/Makefile pi@<PI_IP>:~/carfluxoid-build/kernel/
ssh pi@<PI_IP> 'make -C /lib/modules/$(uname -r)/build M=$HOME/carfluxoid-build/kernel -j4 modules'
```

*(Rebuild and reinstall `aa_bulk.ko` if you ever upgrade the Pi’s Linux kernel package.)*

---

## 4. Install the Base Android Auto Stack

Upload the binaries, assets, TLS pair, and base installer:

```sh
scp local/raspberrypi/aa-phone local/raspberrypi/catplay-aa-rx \
    local/tls/test-cert.pem local/tls/test-key.pem \
    platforms/raspberrypi/install-base.sh \
    platforms/raspberrypi/carfluxoid-aa.service \
    pi@<PI_IP>:~/carfluxoid-build/
scp assets/mire.h264 assets/waiting/attente-*.h264 pi@<PI_IP>:~/carfluxoid-build/assets/
```

Run the base installer and reboot so `dtoverlay=dwc2,dr_mode=peripheral` activates:

```sh
ssh -t pi@<PI_IP> 'sudo sh ~/carfluxoid-build/install-base.sh && sudo reboot'
```

---

## 5. Configure the Wireless CarPlay Radio & Web UI

Supply your own private MFi identity (`certificate.p7b` and `identity.pk8`) locally — never commit these files to git.

```sh
scp /path/to/certificate.p7b /path/to/identity.pk8 \
    platforms/raspberrypi/prepare-radio.sh \
    platforms/raspberrypi/carfluxoid-radio.service \
    platforms/raspberrypi/carfluxoid-web.service \
    pi@<PI_IP>:~/carfluxoid-build/
ssh -t pi@<PI_IP> 'sudo sh ~/carfluxoid-build/prepare-radio.sh && rm -f ~/carfluxoid-build/certificate.p7b ~/carfluxoid-build/identity.pk8'
```

This creates:
- NetworkManager AP profile `carfluxoid-ap` (`CarFluxoid-Pi_AP`, `192.168.50.2/24`, random WPA2 passphrase in `/etc/carfluxoid/radio.env`)
- Root-only MFi identity & HomeKit pairing store in `/var/lib/carfluxoid/`
- `carfluxoid-web.service` (`http://192.168.50.2`) and `carfluxoid-radio.service`

---

## 6. Install the Projected Dashboard & Diagnostics

```sh
make deploy HOST=pi@<PI_IP>
```

On the first run, `install-dashboard.sh` installs `python3-pil`, `ffmpeg`, and `fonts-dejavu-core` if missing, verifies `SHA256SUMS`, runs `catplay-aa-rx --self-test`, tests dashboard H.264 rendering, installs the systemd services (`carfluxoid-dashboard`, `carfluxoid-car-diag`), and restarts the bridge cleanly.

---

## 7. In-Car Use

1. Plug a micro-USB **data** cable from your car’s Android Auto USB port into the Pi’s **USB** port (closest to mini-HDMI).
2. Wait ~15 s for boot; Android Auto launches automatically on the head unit.
3. By default, **auto-start** switches to CarPlay after 2 s (tap any button on the menu during those 2 s to stay in the settings menu).
4. Pair your iPhone with Bluetooth device **CarFluxoid** and accept CarPlay.
5. Open `http://192.168.50.2` while connected to CarPlay Wi-Fi to customize sidebar position, theme (`Auto` follows car headlights), FPS, custom vehicle icon, view live GPS/diagnostics, or upload OTA updates.
