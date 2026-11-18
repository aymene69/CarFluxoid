# Vehicle startup candidate (not yet validated in a car)

This candidate keeps the tested non-B V821 beta kernel and bootloader. It does
not update the device to CatPlay 0.30.0. New upstream boot/kernel assets require
separate validation and an exactly matching module build.

Changes:

- Restore UTC before waiting for D-Bus/USB, using the newer of the saved epoch
  and the firmware build epoch. A saved epoch is a lower bound, not a clock that
  runs while power is removed. iAP2 DeviceTimeUpdate remains the live source.
- Checkpoint valid system time once at service startup and hourly, atomically.
  `/tmp/aa-clock-status` distinguishes seed restoration and persistence errors.
- Run wireless reception in a separate supervisor, regardless of Android Auto.
- Restart failed AA sessions; no 45-second display-negotiation deadline.
- Handle AOA EP0 requests 51/52/53, including requests before configuration.
  Request 53 triggers a delayed disconnect/reconnect as `18d1:2d00`.
- Default launch mode `negotiated` starts with test identity `18d1:4ee1`.
  This is a development identity, not a registered CarFluxoid product VID/PID.
  To compare with direct accessory presentation, put `direct` in
  `/persist/aa-settings/usb-mode`; remove it or put `negotiated` to restore AOA.
  Direct accessory enumeration is allowed by AOA; adding negotiation does not
  establish that this was the cause of the vehicle failures.
- Bounded vehicle snapshots in `/persist/aa-diagnostics/current.log` and
  `previous.log`: start and 15/45/105/285 seconds after startup. No credentials.
  Two records are retained so the next Mac boot does not discard the car record.

Validation completed: RV32 module compilation, host AOA request validation,
supervisor restart regression, clock restoration/checkpoint tests, protocol,
RV32 runtime/settings and offline image/flash tests, 4 KiB EROFS round trip.
Pending: physical AOA control transfers, real USB resets/re-enumeration,
new radio supervisor on hardware, power behavior and projection in Fiat/Kia.
The old `hardware_tested_module_text_sha256` remains recorded alongside the new
compiled reference. A successful compile is not a car-compatibility result.

After a car attempt, retrieve both snapshots immediately on the next Mac boot:

```sh
ssh root@192.168.50.2 'cat /persist/aa-diagnostics/previous.log; cat /persist/aa-diagnostics/current.log'
```

Do not use a live-rootfs SSH flash after the observed `Bus error`. The prepared
candidate installation uses FEL, two reads, rootfs-only writing and full readback.

The pinned kernel has `CONFIG_MODULE_UNLOAD` disabled. Once `aa_bulk` is loaded,
a RAM module replacement cannot work; `rmmod` returns ENOSYS. Use a cold boot
with the candidate image (FEL installation) rather than retrying module removal.

Hardware observations (2026-11-15): DHU confirmed AOA protocol 1, accessory
re-enumeration, USB attach and AA version 1.7. TLS/video are not yet validated
on this candidate. A later boot restored the installed pre-fix sender and showed
stale Unix sockets and absent org.bluez. The current image includes sender EOF
and socket restart fixes plus bounded start attempts for an absent bluetoothd.

## Vehicle stops before AA version negotiation

The negotiated profile uses 18d1:4ee1 with a vendor-specific interface, not a
complete MTP implementation. A successful DHU run with explicit AOA initialization
does not prove that an OEM receiver will probe this initial presentation.
There is no USB mass-storage interface in aa_bulk.

Compare the existing direct accessory profile without flashing or unloading:
`sh scripts/set-usb-mode.sh direct` while connected to the dongle Wi-Fi, then
unplug/replug in the vehicle. This advertises 18d1:2d00 immediately. To restore,
run the same command with `negotiated` and unplug/replug. This is a diagnostic
comparison, not a confirmed Uconnect fix. Record pre_accessory, aoa_requests,
rx_bytes, tx_bytes and phone.log for both profiles. New boot diagnostics include
these counters automatically; installed older firmware can read sysfs directly.

## 2026-11-17 startup candidate

Vehicle report boot 15 confirms GET_PROTOCOL completion status 0 with two bytes,
then no bulk RX/TX. This does not identify a certificate problem or prove that
AOA 1 was refused. Boot 13/14 only retained early snapshots; the reason the boot
or collection stopped is not established.

USB startup no longer waits for D-Bus. Only the wireless receiver waits for its
own D-Bus socket. Early diagnostic data is buffered in RAM for eight seconds
before writing the journal to persist, reducing flash activity during hardware
initialization. If power is lost before that flush, that early buffer is lost.
Three bounded boot reports remain available across unplug/replug. The gadget
logs the first 64 vendor setup probes and explicit GET_PROTOCOL response bytes;
AOA support is still version 1, with no unimplemented HID support advertised.

Host tests cover startup without D-Bus, delayed radio startup, retained boot
reports and size limits. Physical cold-boot reliability and OEM acceptance are
pending hardware validation. The candidate is separate from the installed image.

## AA USB compatibility experiment (2026-11-17)

Reference: aa-proxy/aa-proxy-rs, src/usb_functionfs.rs, handle_ep0_event.
That implementation replies 02 00 to GET_PROTOCOL, acknowledges SET_AUDIO_MODE,
and retains an optional real MTP legacy stage. Our previous vehicle trace replies
01 00 then receives no more requests or AA data. The new candidate defaults the
launcher to AA transport profile 2, acknowledges only SET_AUDIO_MODE(0), and keeps
profile 1 available using /persist/aa-settings/aoa-protocol or AA_AOA_PROTOCOL.
The kernel parameter aoa_protocol accepts only 1/2 and is readable/writable.

This is an AA-specific compatibility experiment, not a claim of general Android
AOAv2 conformance: HID registration/events and USB Audio mode 1 are not supported
and are stalled explicitly, not silently acknowledged. No unimplemented MTP
interface is newly advertised. The user's direct/negotiated USB setting remains
unchanged. Only a new OEM hardware trial can establish whether profile 2 changes
Uconnect's decision after GET_PROTOCOL. The old AOA1 image is retained.

The persistent journal also uses BusyBox-compatible dd instead of head -c to
retain its beginning when bounded; head -c is disabled in this firmware build.

## MTP-to-AOA candidate (2026-11-17)

The AA-specific profile-number experiment did not resolve Uconnect: GET_PROTOCOL
02 00 completed with status 0 and no bulk traffic followed. The new frozen MTP
candidate presents a still-image/MTP interface 06/01/01 with bulk IN/OUT and an
interrupt notification endpoint at 18d1:4ee1. A small read-only responder serves
DeviceInfo, session open/close, and an empty storage/object list. Unsupported
operations return OperationNotSupported; no files are exposed. AOA START causes
a disconnect, FIFO reset, replacement of the function descriptors with the
vendor-specific two-endpoint accessory interface, and reconnect at 18d1:2d00.

The same AA sender retains its video listener across the transition; it does not
start the AA USB reader until accessory_active and usb_online are both true.
This avoids starting bulk reads against endpoints disabled during re-enumeration.
The candidate image pins usb-profile=legacy; ordinary builds without the package
variable CARFLUXOID_USB_PROFILE retain the user's existing USB profile setting.

Software validation includes C MTP response/state/transaction tests and the real
RV32 binary over a PTY (fragmented commands, empty storage, disconnect, endpoint
readiness and preservation of the video socket). This does not emulate the UDC:
real descriptor enumeration and AOA switching must still be checked on hardware.
A dedicated Mac probe verifies MTP DeviceInfo before launching the DHU initializer.

This does not fix or explain the separate kernel Oops in process ip observed
before aa_bulk loaded. The beta kernel, bootloader and persist remain unchanged.

## MTP timeout fallback

The Uconnect trace opens session (1002), reads DeviceInfo (1001), and requests
StorageIDs (1004). All return OK; RX=40 and TX=269. No SEND_STRING/START follows.
Like aa-proxy-rs with require_accessory_start=false, the next candidate falls
back to accessory presentation after a bounded MTP stage instead of waiting
forever. The launcher defaults to 8000 ms for legacy mode only; set
AA_MTP_FALLBACK_MS=0 to disable it. Host-issued START cancels that timer. USB
disable cancels it, and an already-active accessory ignores repeated switches.
The timeout shares the same descriptor-switch worker used by the previously
hardware-tested host-issued START; its timer path still needs OEM validation.
The Mac probe now explicitly reports when fallback already entered accessory
mode rather than falsely claiming a new DeviceInfo check.

USB reads/writes now retain an open-time connection generation. Disable advances
the generation and clears FIFO under the RX lock, waking old readers with ENODEV
even if reconfiguration finishes before their task gets scheduled. This prevents
the MTP reader from consuming the new connection's first AA bytes. Real UDC
handoff/timer behavior remains pending hardware verification.

## CarPlay audio window

Car trial of the attente-iphone candidate: CarPlay video and touch work, no
sound. The Uconnect media sink answers the audio SETUP with CONFIG
`08 02 10 80 20 18 00` (HEADUNIT, max_unacked=4096, index 0). The sender treated
any window above 32 as invalid and fell back to 1 (`AUDIO_READY ... window=1`),
so only one 20 ms PCM block was ever in flight and the sink never received a
continuous stream, although audio focus was granted and CarPlay PCM arrived.
The sender now uses the announced window (bounded 1..4096), resets a window
that stays full for 2 s without acknowledgement, counts underflow notifications
(0x800b), and traces `AUDIO_STATS` every 5 s (blocks sent, acks, window,
underflows, PCM peak level, focus). `tests/audio_window.c` replays the captured
CONFIG. Candidate: `compatibilite-son-20261117`; only aa-phone-v821 changes.
In the next car log, look for `AUDIO_READY window=4096`, then `AUDIO_STATS` with
a growing `sent` and `peak` above 0 while music plays.

Correction after the 2026-11-18 car trial: still no sound. The window stayed at
1 because the START reply was written into the receive buffer before
max_unacked was read. The window was not the cause of the silence anyway: the
new counters show every PCM block acknowledged, real music levels (peak about
30000) and focus granted. The sender emitted START at channel setup, before any
audio focus, and PCM before the grant. Google's integration order is focus
GAIN, START with a new session, PCM, then STOP, and the Uconnect routes AA media
from its playbackStart callback. Candidate `compatibilite-son-v2-20261118`
follows that order. It parses CONFIG before replying, discards PCM until focus
is granted, sends STOP after 2 s of silence, on focus loss and on CarPlay
disconnect, and uses a new session id per stream. `tests/audio_flow.c` drives
the sequence against a fake head unit.
