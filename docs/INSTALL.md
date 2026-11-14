# From a new compatible dongle to CarFluxoid

## Supported hardware

The implemented profile targets the tested **Allwinner V821 non-B revision + RTL8733BS + 16 MiB SPI NOR**, including the corresponding V821/RTL8733BS Carlinkit Mini Ultra variant. A commercial model name alone does not identify the PCB: X1600, V851 and AIC8800 variants are not interchangeable. Confirm the radio/board before purchase or opening the flashing procedure. FEL checks the SoC revision and flash geometry; it cannot identify an unpowered radio chip by its marketing name.

This procedure builds the CarFluxoid bridge, dependencies and USB module from pinned source. It retains the pinned CatPlay issue 56 **hstimer-off** beta base for the kernel, vendor init flow and radio blobs. It is not a from-source replacement for every proprietary component of the dongle.

## Host requirements

Linux or macOS, Docker with at least 4 GiB memory, Python 3.12+, Git, OpenSSL, Make, a C compiler and libusb/pkg-config. On macOS, `brew install libusb pkg-config` provides the native FEL build dependencies. On Debian/Ubuntu use `sudo apt install build-essential libusb-1.0-0-dev pkg-config openssl python3-venv`. USB FEL access on Linux may require running the device commands through sudo with the same virtualenv Python path. Docker compiles binaries; the native host xfel accesses USB, which also works on macOS where Docker USB passthrough is unavailable.

Allow several GiB of disk and time for the first SDK build. Subsequent builds reuse the named Docker build volume. The compiler/tool versions and source archive checksums are tracked in `tooling/sources.lock.json`. No files from the original author's workstation are needed.

## 1. Fetch and compile

```sh
git clone --recurse-submodules https://github.com/aymene69/CarFluxoid.git
cd CarFluxoid
python3 -m venv .venv
.venv/bin/pip install -r requirements.txt
.venv/bin/python carfluxoid.py fetch
.venv/bin/python carfluxoid.py sdk
.venv/bin/python carfluxoid.py build
.venv/bin/python carfluxoid.py xfel-build
```

`build` downloads an explicitly pinned, already published Android Auto reference TLS identity into `local/tls/`; it never retrieves or uploads anyone's MFi private identity. This reference certificate expires on **31 December 2026** and acceptance varies by receiver. To use another valid local pair, put it at `local/tls/test-cert.pem` and `local/tls/test-key.pem` before building. The tool checks matching keys and expiry. A self-signed pair is not assumed to work with Google DHU.

Artifacts are generated under `local/artifacts/`. The builder checks RV32 ELF architecture, rootfs size, EROFS 4 KiB blocks and a complete extract/recompare before producing the partition. Its launcher is the only replaced original beta file. Waiting images and all bootstrap files are assembled automatically.

## 2. Back up the new dongle in FEL

For a dongle with a physical FEL button, unplug it, hold that button while plugging into the host, then release it. Otherwise the original cpbox-abroad firmware has the upstream OTA bootstrap path:

```sh
# Start with Internet available. The upstream helper asks when to switch Wi-Fi.
.venv/bin/python carfluxoid.py stock-to-fel --preset carlinkit
```

The available upstream presets are `carlinkit`, `wooboobox`, `ekiy`. Their vendor-firmware IPs are preset-specific. After CatPlay is installed, normal SSH is **192.168.50.2**; do not reuse the vendor IP for it.

Once FEL is active:

```sh
.venv/bin/python carfluxoid.py fel-backup --output local/original-stock.bin
```

The tool refuses other SoC revisions/flash geometry and reads the complete 16 MiB twice. Both copies are preserved with private permissions. Keep `original-stock.bin` **and** `original-stock.bin.verification` somewhere safe; they contain device-specific data. Do not share them or add them to Git.

## 3. Install the pinned beta base once

Skip this step if the dongle already runs the supported beta normally. The next command writes the pinned base boot/kernel/rootfs and preserves the exact persist bytes from the verified backup. It first takes another double backup, checks it is still the supplied original device state, and performs a complete readback after writing.

```sh
.venv/bin/python carfluxoid.py base-install \
  --stock-backup local/original-stock.bin --output local/base-install
```

Keep USB powered until readback succeeds. Replug and connect the host to the beta Wi-Fi. The name/password are those supplied by the base/configuration; the receiver uses the actual firmware Wi-Fi configuration. The Bluetooth receiver is called CarFluxoid after the bridge is installed. This base-install path is implemented but still needs a fresh factory device hardware acceptance test; don't interpret offline tests as that hardware test.

## 4. Compose with this dongle's data and install the bridge

```sh
.venv/bin/python carfluxoid.py backup --output local/beta-backup.bin
.venv/bin/python carfluxoid.py compose \
  --backup local/beta-backup.bin --output local/CarFluxoid.c2aflash
.venv/bin/python carfluxoid.py flash \
  --image local/CarFluxoid.c2aflash --output local/bridge-install
```

The tool accepts only the known beta kernel hashes, including the previously tested two-byte MAC variant. The rootfs partition is replaced; this dongle's boot/kernel and fresh persist data are retained. A new double backup is made immediately before upload. The upload hash is verified, flash failure stops the procedure, and normal-boot readback checks the written boot/kernel/rootfs before reporting success. If Wi-Fi is unavailable during readback, use the FEL readback/recovery tools instead of assuming success.

An alternative when already in FEL is `fel-flash --image local/CarFluxoid.c2aflash --output local/bridge-fel-install`. This writes only the rootfs and verifies all 16 MiB afterward.

## 5. Validate the same development stage

Connect the dongle by USB to Google DHU, keep LIVI and other receivers closed, pair the iPhone to CarFluxoid and accept CarPlay. Check video, music, touch, then disconnect/reconnect the iPhone. The waiting screen should return. Open `http://192.168.50.2` on the dongle Wi-Fi to change settings. Resolution needs a dongle restart; scale and CarPlay options need a CarPlay reconnection.

Uconnect and microphone/Siri still need separate hardware validation. Reproducing the project stage does not mean every head unit is supported.

## Recovery: exact original firmware

Put the same dongle back in FEL, then:

```sh
.venv/bin/python carfluxoid.py restore \
  --image local/original-stock.bin --output local/restore-original
```

This is explicitly a full 16 MiB restore, including the original persist data. It requires the identical verification copy from the original double backup, takes a new double backup before writing and verifies the complete readback. It does not use another owner's firmware image.
