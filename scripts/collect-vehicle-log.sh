#!/bin/sh
# Snapshot after a vehicle attempt; no service restart, flash or secret material.
set -eu
host=${1:-192.168.50.2}
case "$host" in *[!a-zA-Z0-9.:-]*|'') echo 'Invalid host' >&2;exit 2;; esac
out=${2:-carfluxoid-vehicle-$(date +%Y%m%d-%H%M%S).log}
umask 077
ssh -o ConnectTimeout=8 "root@$host" 'sh -s' > "$out.part" <<'REMOTE'
set -eu
dir=/tmp/aa-diagnostics
mkdir -p "$dir"
umask 077
report="$dir/rapport-complet.log"
{
 echo 'CARFLUXOID VEHICLE SNAPSHOT';date -u;cat /proc/uptime
 uname -a;cat /proc/cmdline
 echo '=== USB MODE AND COUNTERS ==='
 cat /persist/aa-settings/usb-mode 2>/dev/null || true
 for n in pre_accessory mtp_fallback_ms legacy_mtp accessory_active usb_online aoa_protocol aoa_requests aoa_completed aoa_errors aoa_unsupported aoa_last_status rx_bytes tx_bytes;do
  p=/sys/module/aa_bulk/parameters/$n
  if [ -r "$p" ];then printf '%s=' "$n";cat "$p";fi
 done
 for p in /sys/class/udc/*/state /sys/class/udc/*/function /sys/class/udc/*/current_speed /sys/class/usb_role/*/role;do
  if [ -r "$p" ];then echo "$p";cat "$p";fi
 done
 echo '=== RESOURCES ===';cat /proc/meminfo;df -k /tmp /persist;cat /proc/modules
 echo '=== CLOCK ===';cat /tmp/aa-clock-status /persist/aa-clock-epoch 2>/dev/null || true
 for p in /tmp/aa-autoboot/boot.log /tmp/aa-autoboot/session.log /var/log/aa-bridge-boot.log /tmp/aa-autoboot/phone.log /tmp/aa-autoboot/carplay.log /var/log/bluetooth.log /persist/aa-diagnostics/current.log /persist/aa-diagnostics/previous.log;do
  echo "=== $p (last 65536 bytes) ==="
  if [ -f "$p" ];then tail -c 65536 "$p";else echo MISSING;fi
 done
 echo '=== KERNEL (last 262144 bytes) ===';dmesg | tail -c 262144
} > "$report.new"
mv -f "$report.new" "$report"
cat "$report"
REMOTE
mv -f "$out.part" "$out"
printf 'Rapport enregistré : %s\n' "$out"
