#!/bin/sh
# Truncate in place: open stdout/stderr descriptors continue writing to the same inode.
# Check every 10 seconds; one interval of output may exceed the 64 KiB threshold.
while :; do
 for log in /tmp/aa-autoboot/phone.log /tmp/aa-autoboot/carplay.log /var/log/aa-bridge-boot.log /var/log/aa-settings-web.log /var/log/bluetooth.log; do
  [ -f "$log" ] || continue
  size=$(wc -c < "$log")
  if [ "$size" -gt 65536 ]; then
   : > "$log"
   printf '%s\n' 'LOG_TRUNCATED: older entries discarded (64 KiB threshold)' >> "$log"
  fi
 done
 sleep 10
done
