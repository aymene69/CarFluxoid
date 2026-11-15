#!/bin/sh
set -eu
# Restore time and start boot diagnostics before waiting for USB or radios.
export AA_CLOCK_SYNC=1
/bin/sh /usr/lib/aa-bridge/clock-service.sh --restore
setsid /bin/sh /usr/lib/aa-bridge/clock-service.sh </dev/null >>/var/log/aa-bridge-boot.log 2>&1 &
setsid /bin/sh /usr/lib/aa-bridge/vehicle-log.sh </dev/null >/dev/null 2>&1 &
# Release the stock Apple presentation early: the car must see our Android role.
for iphone in /sys/class/iphone/*; do
 if [ -w "$iphone/bind" ];then
  if echo 0 > "$iphone/bind";then echo "AA_STOCK_USB_RELEASED: $iphone";else echo "AA_STOCK_USB_RELEASE_FAILED: $iphone";fi
 fi
done
i=0
while [ "$i" -lt 90 ]; do
 if [ -S /run/dbus/system_bus_socket ]; then break;fi
 i=$((i+1));sleep 1
done
[ "$i" -lt 90 ] || { echo 'AA_BOOT_FAILED: D-Bus unavailable (see persist diagnostics)';exit 1; }
export AA_DIAGNOSTIC=${AA_DIAGNOSTIC:-0}
work=/tmp/aa-autoboot
mkdir -p "$work"
for n in catplay-aa-rx aa-phone-v821 aa_bulk.ko;do
 gunzip -c "/usr/lib/aa-bridge/$n.gz" > "$work/$n"
done
cp /usr/lib/aa-bridge/test-cert.pem /usr/lib/aa-bridge/test-key.pem /usr/lib/aa-bridge/mire.h264 /usr/lib/aa-bridge/wifi-env.sh /usr/lib/aa-bridge/usb-test.sh "$work/"
chmod 700 "$work" "$work/catplay-aa-rx" "$work/aa-phone-v821"
chmod 600 "$work/test-key.pem" "$work/wifi-env.sh"
for screen in /usr/lib/aa-bridge/attente-*.h264; do
 [ ! -f "$screen" ] || ln -sf "$screen" "$work/"
done
setsid /bin/sh /usr/lib/aa-bridge/log-maintenance.sh </dev/null >/dev/null 2>&1 &
setsid /bin/sh /usr/lib/aa-bridge/web-service.sh </dev/null >/dev/null 2>&1 &
export AA_RADIO_EXTERNAL=1
setsid /bin/sh /usr/lib/aa-bridge/radio-service.sh </dev/null >>/var/log/aa-bridge-boot.log 2>&1 &
echo 'AA_BOOT_READY: launching bridge; beta kernel unchanged'
exec /bin/sh "$work/usb-test.sh"
