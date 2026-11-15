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
