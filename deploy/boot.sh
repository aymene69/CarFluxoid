#!/bin/sh
set -eu
# Started by the original CatPlay service after the beta boot flow initializes radio.
i=0
while [ "$i" -lt 90 ]; do
 if [ -S /run/dbus/system_bus_socket ] && grep -q -i iphone /sys/class/udc/*/function 2>/dev/null; then break;fi
 i=$((i+1));sleep 1
done
[ "$i" -lt 90 ] || { echo 'AA_BOOT_FAILED: beta radio/gadget initialization incomplete';exit 1; }
work=/tmp/aa-autoboot
mkdir -p "$work"
for n in catplay-aa-rx aa-phone-v821 aa_bulk.ko;do
 gunzip -c "/usr/lib/aa-bridge/$n.gz" > "$work/$n"
done
cp /usr/lib/aa-bridge/test-cert.pem /usr/lib/aa-bridge/test-key.pem /usr/lib/aa-bridge/mire.h264 /usr/lib/aa-bridge/wifi-env.sh /usr/lib/aa-bridge/usb-test.sh "$work/"
chmod 700 "$work" "$work/catplay-aa-rx" "$work/aa-phone-v821"
chmod 600 "$work/test-key.pem" "$work/wifi-env.sh"
for screen in /usr/lib/aa-bridge/attente-*.h264; do
 [ ! -f "$screen" ] || cp "$screen" "$work/"
done
setsid /bin/sh /usr/lib/aa-bridge/web-service.sh </dev/null >/dev/null 2>&1 &
echo 'AA_BOOT_READY: launching bridge; beta kernel unchanged'
exec /bin/sh "$work/usb-test.sh"
