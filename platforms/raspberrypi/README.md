# Raspberry Pi Zero 2 W — experimental ARM64 target

CarFluxoid runs on a Raspberry Pi Zero 2 W using Raspberry Pi OS Lite 64-bit. The bridge projects CarPlay through the Pi's micro-USB data port and provides an on-screen menu and local web settings. Video, audio and touch were exercised with an iPhone and Google DHU on a physical Zero 2 W. See the validation notes below for current limits.

[Projected dashboard and controls](dashboard/README.md)

This directory does not replace the validated V821 firmware profile. Never flash a V821 SPI image onto a Pi SD card.

## Development image

Use Raspberry Pi OS Lite 64-bit, enable SSH and initially join a 2.4 GHz home network. Connect a data cable to the micro-USB port labelled USB, nearest mini-HDMI. PWR IN carries power only. The port is configured as a USB peripheral with `dtoverlay=dwc2,dr_mode=peripheral` under `[all]` in `/boot/firmware/config.txt`.

## Build on an ARM64 Linux Docker host

Start from the repository SDK image (`tooling/Dockerfile`, Rust version pinned in `tooling/sources.lock.json`). This Pi target uses the same source and cryptographic library revision as the dongle, but its own output directory. The Pi build script downloads and verifies its MbedTLS source itself; it does not need the V821 sysroot.

```sh
git submodule update --init --recursive
./scripts/prepare-catplay.sh
docker build -t carfluxoid-sdk:2026-11-14 -f tooling/Dockerfile .
docker build -t carfluxoid-pi-build:20261118 -f platforms/raspberrypi/Dockerfile .
docker run --rm -v "$PWD:/input" -v carfluxoid-pi-build:/build carfluxoid-pi-build:20261118 sh /input/platforms/raspberrypi/build.sh
```

Output: `local/raspberrypi/aa-phone`, `catplay-aa-rx`, `SHA256SUMS`. This is a native ARM64 container build; an x86 host needs an ARM64 container/emulation configuration.

The kernel module must match the **running Pi kernel**, not the V821 kernel. Copy `bridge-aa/kernel/{aa_bulk.c,aa_aoa.h,Makefile}` to `~/carfluxoid-build/kernel` on the Pi, then run:

```sh
make -C /lib/modules/$(uname -r)/build M=$HOME/carfluxoid-build/kernel -j4 modules
```

This needs the matching kernel headers and the system C compiler. Rebuild after a kernel update. If headers are absent, install the Raspberry Pi OS headers package matching the running kernel before proceeding.

## Staging and installation

Create `~/carfluxoid-build` on the Pi containing:

- `aa-phone`, `catplay-aa-rx` (ARM64 output)
- `kernel/aa_bulk.ko` (built on this Pi)
- `assets/mire.h264` and `assets/attente-*.h264` from the repository
- `test-cert.pem`, `test-key.pem` from the repository's local developer TLS setup
- `install-base.sh`, `carfluxoid-aa.service` from this directory

To prepare the local Android Auto TLS pair, install the repository's Python requirements and run `python3 carfluxoid.py tls`. Copy the pair from `local/tls/`. Use a valid pair accepted by your head unit. See the [installation guide](../../docs/INSTALL.md) for details.

Run `sudo sh ~/carfluxoid-build/install-base.sh`, then reboot. The installer saves a copy of `config.txt`, enables the Android Auto service, and keeps new system journals in RAM with an 8 MiB limit. Check it with `systemctl status carfluxoid-aa`. The base installation provides the Android Auto waiting picture.

## Wireless receiver

The receiver needs a valid private MFi identity supplied by the installer. Keep identities and pairing data local.

Add owner-provided `certificate.p7b` and `identity.pk8`, plus `prepare-radio.sh`, `carfluxoid-radio.service` and `carfluxoid-web.service` to the staging directory. Run `sudo sh ~/carfluxoid-build/prepare-radio.sh`, then remove the staged identity copies after success. The installed identity and pairing store are root-only. This command prepares, but does not activate, the AP profile or wireless receiver.

The default AP is **CarFluxoid-Pi_AP** at **192.168.50.2**, with NetworkManager profile `carfluxoid-ap`. A first installation generates a Wi-Fi passphrase in `/etc/carfluxoid/radio.env`, readable by root. The default Bluetooth name is **CarFluxoid**.

Install the dashboard below to activate the AP from the projected menu. For manual use, install `start-ap-trial.sh` and `return-home.sh` under `/opt/carfluxoid/bin/` with executable permissions. Start the AP from an SSH session on the Pi:

```sh
sudo systemd-run --collect /opt/carfluxoid/bin/start-ap-trial.sh
```

The SSH session on the home IP disconnects when the AP activates. The AP remains active until you change networks or reboot. To return to the home Wi-Fi, find its UUID with `nmcli -f NAME,UUID connection show` and run `sudo /opt/carfluxoid/bin/return-home.sh HOME_UUID`. For SSH or web diagnostics, join the AP and use `ssh YOUR_USER@192.168.50.2` or `http://192.168.50.2`.

## Install the projected dashboard

After the base and radio setup, assemble a staging directory on the build host from the repository root:

```sh
bundle=$(mktemp -d)
cp local/raspberrypi/aa-phone local/raspberrypi/catplay-aa-rx "$bundle/"
cp platforms/raspberrypi/install-dashboard.sh platforms/raspberrypi/carfluxoid-dashboard.service "$bundle/"
cp platforms/raspberrypi/start-ap-trial.sh platforms/raspberrypi/start-radio.sh "$bundle/"
cp platforms/raspberrypi/dashboard/dashboard.py tooling/waiting_iphone_config.py "$bundle/"
(cd "$bundle" && shasum -a 256 aa-phone catplay-aa-rx install-dashboard.sh carfluxoid-dashboard.service \
  start-ap-trial.sh start-radio.sh dashboard.py waiting_iphone_config.py > SHA256SUMS)
```

On Linux, `sha256sum` can replace `shasum -a 256`. Copy the staging directory to the Pi and run its installer with `sudo sh /path/to/staging/install-dashboard.sh`. Keep the Pi on a network with Internet access during the first dashboard install so APT can install Pillow and FFmpeg if needed. The installer verifies the manifest, tests image generation and saves the previous programs in `/opt/carfluxoid/dashboard-backup.*` before restarting the services.

Start Google DHU over USB, then choose **Démarrer CarPlay**. Pair the iPhone via Bluetooth and accept CarPlay. Use **Retour au menu** from the connection screen or the vehicle tile in CarPlay to reopen the CarFluxoid menu.

## Scheduling and resource use

The C transport already has independent USB-reading and processing threads. `AA_RUNTIME_WORKERS=4` opts the Pi wireless receiver into a four-worker Tokio runtime; the dongle default remains one worker. This allows concurrent work but does not promise fourfold performance. No fixed CPU affinity or realtime priority is imposed without measurements. The Pi radio is 2.4 GHz; shared Wi-Fi/Bluetooth airtime remains a performance factor.

## Validation

- Eight native C test programs passed, including audio_flow, AOA, video configuration and reader lifecycle.
- Six receiver Rust tests passed in the ARM64 build environment.
- All 56 HAP cryptographic/library tests passed on the physical Zero 2 W. Docker initially blocked eight AF_ALG tests with EPERM; those same tests passed on the Pi.
- Receiver self-test passed on the Pi; USB module loaded and device node appeared after reboot.
- On the physical Pi, Google DHU displayed the menu and the iPhone projection. Audio and touch were exercised.
- A local DHU-over-TCP test exercised menu startup, the connection screen, **Retour au menu**, and returning to the menu after simulated iPhone video ended.
- The ARM64 test suite verified the web-uploaded artwork in the serialized CarPlay response. The iPhone displayed the custom Fiat tile after CarFluxoid was removed and paired again. The latest Wi-Fi automation still requires a hardware check.

Sustained audio quality, microphone reliability, steering-wheel controls and real head unit coverage remain under test.
