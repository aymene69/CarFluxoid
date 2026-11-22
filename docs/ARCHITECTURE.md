# Architecture

CarFluxoid splits responsibilities cleanly across four isolated components communicating over Unix domain sockets in `/run/carfluxoid` (preserved across restarts via `RuntimeDirectoryPreserve=yes`).

---

## Components

### 1. Kernel USB Gadget (`bridge-aa/kernel/aa_bulk.c`)
- Registers a USB High-Speed composite gadget (`18d1:2d00`, AOA transport profile 2) on the Raspberry Pi Zero 2 W’s DWC2 peripheral controller (`dtoverlay=dwc2,dr_mode=peripheral`).
- Prequeues 8×4 KiB bulk OUT requests into a 128 KiB kernel FIFO so head-unit transfers never stall while userspace handles TLS/video/audio.
- Tracks per-open USB connection generations so USB resets wake blocked readers cleanly with `-ENODEV`.

### 2. Android Auto Transport (`bridge-aa/src/aa_phone.c`)
- Waits for `usb_online=1`, opens `/dev/aa_bulk`, starts a realtime `SCHED_FIFO` reader thread, and negotiates AA v1.7 + TLS 1.2 (MbedTLS).
- Parses the head unit’s `ServiceDiscoveryResponse` and opens all supported channels:

| AA Channel (Uconnect VP2) | Service | Direction | Purpose |
| :---: | :--- | :---: | :--- |
| `ch=0` | Control | Bi-dir | Version, TLS, Ping/Pong, Audio Focus, Voice Session (`0x11`), ByeBye |
| `ch=1` | VideoSink | Pi → HU | H.264 Annex-B video (`MEDIA_CODEC_CONFIG` + POC type-2 slice rewrite) |
| `ch=2` | InputSource | HU → Pi | Touchscreen & unpacked proto2 steering-wheel/rotary key bindings |
| `ch=3` | MediaSource (Mic) | HU → Pi | 16 kHz mono PCM16 car microphone capture |
| `ch=4` | MediaAudioSink | Pi → HU | 48 kHz stereo PCM16 main audio (`window=4096`, pre-roll + focus FSM) |
| `ch=5` | GuidanceAudioSink | Pi → HU | 16 kHz mono PCM16 navigation/Siri prompts (`GAIN_TRANSIENT_MAY_DUCK`) |
| `ch=7` | SensorSource | HU → Pi | `NIGHT_MODE` (`10`), `LOCATION` (`1`), `SPEED` (`3`) |
| `ch=9` | Bluetooth | HU → Pi | Head unit Bluetooth MAC extracted from SDR (`video.sock.btmac`) |
| `ch=11` | MediaPlaybackStatus | Pi → HU | Playback state (`0x8001`) and NowPlaying track metadata (`0x8003`) |

### 3. Wireless CarPlay Receiver & Web Server (`bridge-aa/carplay-rx/`)
- Built on the pinned `catplay/` stack with `patches/catplay.patch` (4-worker Tokio runtime, RTP reorder recovery, non-blocking iAP2 flush, isolated iAP2 error handling).
- **`main.rs`**: AirPlay/CarPlay RTSP session, video passthrough to `video.sock`, main & guidance (`AltAudio`, 3:1 48 kHz stereo → 16 kHz mono decimation) audio sinks, HID touch/media/telephony/rotary/Siri forwarding, OEM icon + safe-area viewport negotiation.
- **`gps_iap2.rs`**: iAP2-over-AirPlay session (`GpsIap2Session` + `GpsHub`) converting car GPS & speed into NMEA 0183 (`$GPGGA`, `$GPRMC`, `$PASCD`), syncing UTC clock from `DeviceTimeUpdate`, and subscribing to `NowPlayingUpdate`.
- **`microphone.rs`**: 16 kHz → 24/48 kHz linear resampler feeding CarPlay’s `AudioRecorderControl` only when CarPlay opens recording.
- **`web.rs` + `assets/settings.html`**: HTTP server on port 80 (`GET /`, `POST /save`, `POST /logo`, `POST /reset`, `GET /diag`, `GET /logs`, `POST /ota` with Origin guard, tar path-traversal guard, `SHA256SUMS` verification, and `--self-test`).

### 4. Projected Dashboard & Diagnostics (`platforms/raspberrypi/`)
- **`dashboard/dashboard.py`**: Renders an 800×480 / 720p / 1080p touchscreen UI into H.264 IDR frames aligned with CarPlay’s SPS/PPS (`tooling/waiting_iphone_config.py`), handles touch events, and triggers automatic CarPlay startup (`start-ap-trial.sh`) after a 2 s settle window.
- **`car-diag.sh`**: Captures bounded snapshots (`+20s`, `+40s`, `+60s`, `+120s`) to `/var/lib/carfluxoid/car-diag.log` (and rotates previous boot to `car-diag.prev.log`).

---

## IPC Socket Map (`/run/carfluxoid/`)

| Path | Type | Producer → Consumer | Payload |
| :--- | :---: | :--- | :--- |
| `video.sock` | `SOCK_STREAM` | `catplay-aa-rx` → `aa-phone` | `[u32 len][u64 pts_us][Annex-B H.264]` |
| `video.sock.audio` | `SOCK_DGRAM` | `catplay-aa-rx` → `aa-phone` | 48 kHz stereo PCM16 blocks |
| `video.sock.guidance` | `SOCK_DGRAM` | `catplay-aa-rx` → `aa-phone` | 16 kHz mono PCM16 guidance blocks |
| `video.sock.mic` | `SOCK_DGRAM` | `aa-phone` → `catplay-aa-rx` | `[u32 rate][mono PCM16]` |
| `video.sock.touch` | `SOCK_DGRAM` | `aa-phone` → `catplay-aa-rx` | Touch (`0/1`), keyframe (`0x10`), HID (`0x20..0x25`), Night (`0x26`), GPS (`0x27`), Speed (`0x28`) |
| `video.sock.control` | `SOCK_DGRAM` | `catplay-aa-rx` / `dashboard.py` → `aa-phone` | Shell/mic events (`1..8`) & NowPlaying metadata (`0x30`) |
| `video.sock.{display,night,btmac,gps,guidance.fmt}` | Files | `aa-phone` → `catplay-aa-rx` / `dashboard.py` | Negotiated head-unit state |
| `session.log` | File | `aa-phone` | Compact `fsync`-persisted AA protocol trace |
