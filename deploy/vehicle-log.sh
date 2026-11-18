#!/bin/sh
# Persistent bounded per-boot diagnostics, independent of the AA session.
# Preserve the beginning and recent history; never record keys or Wi-Fi settings.
# Capture early boot in RAM; avoid diagnostic flash writes during radio/USB init.
early=/tmp/aa-early-boot-$$.log
{ echo '=== EARLY BOOT (RAM) ===';date -u;cat /proc/uptime;dmesg; } > "$early" 2>&1
sleep 8
dir=/persist/aa-diagnostics
mkdir -p "$dir" || exit 0
chmod 700 "$dir"
umask 077
seq=$(cat "$dir/boot-sequence" 2>/dev/null || echo 0)
case "$seq" in ''|*[!0-9]*) seq=0;; esac
seq=$((seq+1))
printf '%s\n' "$seq" > "$dir/boot-sequence.new"
mv -f "$dir/boot-sequence.new" "$dir/boot-sequence"
name=$(printf 'boot-%08d.log' "$seq")
report="$dir/$name"
# Three boots including the new one. Keep the vehicle boot when plugged into a Mac.
set -- "$dir"/boot-*.log
while [ "$#" -gt 2 ];do [ ! -f "$1" ] || rm -f "$1";shift;done
if [ -f "$dir/current.log" ];then
 old=$(readlink "$dir/current.log" 2>/dev/null || true)
 if [ -n "$old" ];then ln -sf "$old" "$dir/previous.log";else mv -f "$dir/current.log" "$dir/previous.log";fi
fi
: > "$report"
ln -sf "$name" "$dir/current.log"
printf 'CARFLUXOID BOOT %s\n' "$seq" >> "$report"
cat "$early" >> "$report"
rm -f "$early"
record(){
 tmp=/tmp/aa-vehicle-snapshot-$$
 {
 echo '=== SNAPSHOT ===';date -u;cat /proc/uptime;cat /proc/cmdline
 for p in /sys/class/udc/*/state /sys/class/udc/*/function /sys/class/udc/*/current_speed /sys/class/usb_role/*/role;do
  if [ -r "$p" ];then echo "$p";cat "$p";fi
 done
 echo USB_AOA_COUNTERS
 for n in pre_accessory mtp_fallback_ms legacy_mtp accessory_active usb_online aoa_protocol aoa_requests aoa_completed aoa_errors aoa_unsupported aoa_last_status rx_bytes tx_bytes;do
  p=/sys/module/aa_bulk/parameters/$n
  if [ -r "$p" ];then printf '%s=' "$n";cat "$p";fi
 done
 echo RESOURCES;head -n 8 /proc/meminfo;df -k /tmp /persist
 echo CLOCK;cat /persist/aa-clock-epoch /tmp/aa-clock-status 2>/dev/null
 for p in /var/log/aa-bridge-boot.log /tmp/aa-autoboot/phone.log /tmp/aa-autoboot/carplay.log /var/log/bluetooth.log;do
  echo "=== $p ===";tail -c 8192 "$p" 2>/dev/null
 done
 echo KERNEL;dmesg | tail -c 16384
 } > "$tmp" 2>&1
 cat "$tmp" >> "$report"
 size=$(wc -c < "$report")
 if [ "$size" -gt 131072 ];then
  { dd if="$report" bs=32768 count=1 2>/dev/null;printf '\n=== BOUNDED LOG: intermediate snapshots omitted ===\n';tail -c 98176 "$report"; } > "$report.new"
  mv -f "$report.new" "$report"
 fi
 rm -f "$tmp"
 sync
}
# Capture the first enumeration quickly, then continue throughout a long car test.
i=0
while :;do
 record
 i=$((i+1))
 if [ "$i" -lt 13 ];then sleep 5;else sleep 30;fi
done
