#!/bin/sh
# Live diagnostics stay in bounded RAM. Only sparse checkpoints touch persist.
# No keys, Wi-Fi credentials or pairing databases are read by this collector.
ram=${AA_LOG_RAM_DIR:-/tmp/aa-diagnostics}
dir=${AA_LOG_PERSIST_DIR:-/persist/aa-diagnostics}
uptime_file=${AA_LOG_UPTIME_FILE:-/proc/uptime}
work=${AA_LOG_WORK_DIR:-/tmp/aa-autoboot}
mkdir -p "$ram" || exit 1
chmod 700 "$ram"
umask 077
report="$ram/current.log"
uptime_seconds(){ read -r up rest < "$uptime_file";printf '%s\n' "${up%%.*}"; }
next_after(){
 if [ "$1" -lt 20 ];then echo 20
 elif [ "$1" -lt 120 ];then echo 120
 elif [ "$1" -lt 600 ];then echo 600
 else echo $(( ($1/1800+1)*1800 ));fi
}
record(){
 tmp="$ram/snapshot-$$"
 {
 echo '=== SNAPSHOT ===';date -u;cat "$uptime_file";cat /proc/cmdline
 for p in /sys/class/udc/*/state /sys/class/udc/*/function /sys/class/udc/*/current_speed /sys/class/usb_role/*/role;do
  if [ -r "$p" ];then echo "$p";cat "$p";fi
 done
 echo USB_AOA_COUNTERS
 for n in pre_accessory mtp_fallback_ms legacy_mtp accessory_active usb_online aoa_protocol aoa_requests aoa_completed aoa_errors aoa_unsupported aoa_last_status rx_bytes tx_bytes;do
  p=/sys/module/aa_bulk/parameters/$n
  if [ -r "$p" ];then printf '%s=' "$n";cat "$p";fi
 done
 echo RESOURCES;head -n 8 /proc/meminfo;df -k "$ram" "$dir" 2>/dev/null
 echo CLOCK;cat /persist/aa-clock-epoch /tmp/aa-clock-status 2>/dev/null
 for p in "$work/boot.log" "$work/phone.log" "$work/carplay.log" "$work/session.log" "$work/web.log" /var/log/bluetooth.log;do
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
}
checkpoint(){
 # Fail closed on low space: keep the last good report and all identity/settings.
 mkdir "$ram/save.lock" 2>/dev/null || return 1
 mkdir -p "$dir" || { rmdir "$ram/save.lock";return 1; }
 available=$(df -k "$dir" | tail -n 1 | awk '{print $4}')
 case "$available" in ''|*[!0-9]*) available=0;; esac
 if [ "$available" -lt 160 ];then
  echo 'LOG_CHECKPOINT_SKIPPED: less than 160 KiB free; live report remains in RAM' >&2
  rmdir "$ram/save.lock";return 1
 fi
 name=$(cat "$ram/persist-name" 2>/dev/null || true)
 number=${name#boot-};number=${number%.log}
 case "$name:$number" in boot-*.log:*) case "$number" in ''|*[!0-9]*) name=;; esac;; *) name=;; esac
 if [ -z "$name" ];then
  seq=$(cat "$dir/boot-sequence" 2>/dev/null || echo 0)
  case "$seq" in ''|*[!0-9]*) seq=0;; esac
  seq=$((seq+1));name=$(printf 'boot-%08d.log' "$seq")
  printf '%s\n' "$seq" > "$dir/boot-sequence.new" && mv -f "$dir/boot-sequence.new" "$dir/boot-sequence" || { rmdir "$ram/save.lock";return 1; }
 fi
 if ! cp "$report" "$dir/$name.new";then rm -f "$dir/$name.new";rmdir "$ram/save.lock";return 1;fi
 sync
 mv -f "$dir/$name.new" "$dir/$name" || { rmdir "$ram/save.lock";return 1; }
 old=$(readlink "$dir/current.log" 2>/dev/null || true)
 if [ "$old" != "$name" ];then
  if [ -n "$old" ];then ln -sf "$old" "$dir/previous.log"
  elif [ -f "$dir/current.log" ];then mv -f "$dir/current.log" "$dir/previous.log";fi
  ln -sf "$name" "$dir/current.log"
 fi
 # Prune only our own boot reports, only after the new one has been written.
 set -- "$dir"/boot-*.log
 while [ "$#" -gt 3 ];do [ ! -f "$1" ] || rm -f "$1";shift;done
 sync
 printf '%s\n' "$name" > "$ram/persist-name"
 uptime_seconds > "$ram/last-save"
 echo "LOG_CHECKPOINT_SAVED: $name" >&2
 rmdir "$ram/save.lock"
}
if [ "${1:-}" = --save ];then
 # Explicit user export can save the newest RAM history before unplugging.
 record;checkpoint;exit $?
fi
lock="$ram/collector.lock"
if ! mkdir "$lock" 2>/dev/null;then
 pid=$(cat "$lock/pid" 2>/dev/null || true)
 if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null;then exit 0;fi
 rm -f "$lock/pid";rmdir "$lock" 2>/dev/null || exit 1
 mkdir "$lock" || exit 1
fi
echo $$ > "$lock/pid"
trap 'rm -f "$lock/pid";rmdir "$lock" 2>/dev/null || true' EXIT
trap 'exit 0' INT TERM
if [ ! -f "$report" ];then
 { echo 'CARFLUXOID LIVE BOOT — sparse persistent checkpoints';echo '=== EARLY BOOT (RAM) ===';date -u;cat "$uptime_file";dmesg; } > "$report" 2>&1
fi
saved=$(cat "$ram/last-save" 2>/dev/null || echo -1)
case "$saved" in ''|*[!0-9-]*) saved=-1;; esac
if [ "$saved" -lt 0 ];then next=20;else next=$(next_after "$saved");fi
while :;do
 record
 now=$(uptime_seconds)
 if [ "$now" -ge "$next" ];then checkpoint || true;next=$(next_after "$now");fi
 if [ "$now" -lt 60 ];then sleep 5;else sleep 30;fi
done
