#!/bin/sh
# Change only the next-boot USB presentation; no flash or module unloading.
set -eu
mode=${1:-}
case "$mode" in direct|negotiated) ;; *) echo "Usage: $0 direct|negotiated [host]" >&2;exit 2;; esac
host=${2:-192.168.50.2}
case "$host" in *[!a-zA-Z0-9.:-]*|'') echo 'Invalid host' >&2;exit 2;; esac
ssh -o ConnectTimeout=8 "root@$host" "sh -s -- $mode" <<'REMOTE'
set -eu
mode=$1
[ -r /usr/lib/aa-bridge/usb-test.sh ] || { echo 'CarFluxoid launcher missing';exit 1; }
grep -q 'pre_accessory=' /usr/lib/aa-bridge/usb-test.sh || { echo 'USB profile unsupported';exit 1; }
echo 'Current USB state:'
for n in pre_accessory mtp_fallback_ms legacy_mtp accessory_active usb_online aoa_protocol aoa_requests aoa_completed aoa_errors aoa_unsupported aoa_last_status rx_bytes tx_bytes;do
 p=/sys/module/aa_bulk/parameters/$n
 if [ -r "$p" ];then printf '%s=' "$n";cat "$p";fi
done
mkdir -p /persist/aa-settings
printf '%s\n' "$mode" > /persist/aa-settings/usb-mode.new
mv /persist/aa-settings/usb-mode.new /persist/aa-settings/usb-mode
sync
[ "$(cat /persist/aa-settings/usb-mode)" = "$mode" ]
echo "Next boot USB mode: $mode. No flash. Unplug/replug the dongle to apply."
REMOTE
