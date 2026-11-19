#!/bin/sh
# Truncate in place: open stdout/stderr descriptors continue writing to the same inode.
# Check every 10 seconds; one interval of output may exceed the 64 KiB threshold.
while :; do
 for log in /tmp/aa-autoboot/phone.log /tmp/aa-autoboot/carplay.log /tmp/aa-autoboot/boot.log /tmp/aa-autoboot/web.log /tmp/aa-autoboot/session.log /tmp/aa-autoboot/session.log.old /var/log/bluetooth.log; do
  [ -f "$log" ] || continue
  size=$(wc -c < "$log")
  if [ "$size" -gt 65536 ]; then
   : > "$log"
   printf '%s\n' 'LOG_TRUNCATED: older entries discarded (64 KiB threshold)' >> "$log"
  fi
 done
 sleep 10
done
