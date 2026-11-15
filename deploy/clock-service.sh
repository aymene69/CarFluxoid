#!/bin/sh
# Saved time is only a boot floor; iPhone DeviceTimeUpdate supplies current UTC.
persist=${AA_CLOCK_FILE:-/persist/aa-clock-epoch}
source=${AA_CLOCK_SOURCE:-/tmp/aa-clock-from-iphone}
valid(){ case "$1" in ''|*[!0-9]*) return 1;; esac; [ "${#1}" -eq 10 ] && [ "$1" -ge 1577836800 ] && [ "$1" -lt 4102444800 ]; }
if [ "${1:-}" = --restore ]; then
 saved=$(cat "$persist" 2>/dev/null || true)
 now=$(date +%s)
 if valid "$saved" && [ "$now" -lt "$saved" ]; then
  date -u -s "@$saved" && echo 'CLOCK_RESTORED: saved lower bound; awaiting iPhone current time'
 fi
 exit 0
fi
last=
while :; do
 epoch=$(cat "$source" 2>/dev/null || true)
 if valid "$epoch" && [ "$epoch" != "$last" ]; then
  if date -u -s "@$epoch"; then
   if (umask 077; printf '%s\n' "$epoch" > "$persist.new" && mv "$persist.new" "$persist"); then
    sync
    echo 'CLOCK_SYNCED: current UTC received from iPhone'
    last=$epoch
   fi
  fi
 fi
 sleep 2
done
