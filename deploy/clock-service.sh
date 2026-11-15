#!/bin/sh
# No battery-backed RTC: restore a lower bound, then apply a connected time source.
persist=${AA_CLOCK_FILE:-/persist/aa-clock-epoch}
source=${AA_CLOCK_SOURCE:-/tmp/aa-clock-from-iphone}
build=${AA_CLOCK_BUILD:-/usr/lib/aa-bridge/build-epoch}
status=${AA_CLOCK_STATUS:-/tmp/aa-clock-status}
valid(){ case "$1" in ''|*[!0-9]*) return 1;; esac; [ "${#1}" -eq 10 ] && [ "$1" -ge 1577836800 ] && [ "$1" -lt 4102444800 ]; }
report(){ printf '%s\n' "$*" > "$status";echo "$*"; }
save(){
 (umask 077; printf '%s\n' "$1" > "$persist.new" && mv "$persist.new" "$persist") || { report 'CLOCK_SAVE_FAILED: persist unavailable or read-only';return 1; }
 sync
}
if [ "${1:-}" = --restore ]; then
 saved=$(cat "$persist" 2>/dev/null || true)
 built=$(cat "$build" 2>/dev/null || true)
 origin=persist
 if valid "$built" && { ! valid "$saved" || [ "$built" -gt "$saved" ]; };then saved=$built;origin=build;fi
 now=$(date +%s)
 if valid "$saved"; then
  if [ "$now" -lt "$saved" ];then
   date -u -s "@$saved" && report "CLOCK_RESTORED: source=$origin epoch=$saved (lower bound, awaiting current time)"
  else report "CLOCK_VALID: epoch=$now";fi
 else report 'CLOCK_NO_SEED: awaiting connected time source';fi
 exit 0
fi
last=
ticks=0
while :; do
 epoch=$(cat "$source" 2>/dev/null || true)
 if valid "$epoch" && [ "$epoch" != "$last" ]; then
  if date -u -s "@$epoch" && save "$epoch"; then
   report "CLOCK_SYNCED: source=iPhone epoch=$epoch"
   last=$epoch
  fi
 fi
 # Checkpoint once at startup and once an hour, never persist 1970.
 if [ "$ticks" -eq 0 ];then
  now=$(date +%s);saved=$(cat "$persist" 2>/dev/null || true)
  if valid "$now" && { ! valid "$saved" || [ "$now" -gt "$saved" ]; };then save "$now" || true;fi
 fi
 ticks=$(((ticks+1)%1800))
 sleep 2
done
