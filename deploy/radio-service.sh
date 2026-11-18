#!/bin/sh
# Wireless CarPlay is supervised independently of the car-side USB session.
work=${AA_RADIO_WORK:-/tmp/aa-autoboot}
cd "$work" || exit 1
mkdir -p "$work/state" || exit 1
chmod 700 "$work/state"
for n in certificate.p7b identity.pk8;do cp "/persist/$n" "$work/state/" || exit 1;done
[ ! -f /persist/homekit_rx_db.bin ] || cp /persist/homekit_rx_db.bin "$work/state/"
chmod 600 "$work/state/"*
. "$work/wifi-env.sh"
export AA_VIDEO_SOCKET="$work/video.sock"
attempts=0
bus=${AA_DBUS_SOCKET:-/run/dbus/system_bus_socket}
echo 'AA_RADIO_WAIT_DBUS: USB continues independently'
while [ ! -S "$bus" ];do sleep 1;done
echo AA_RADIO_DBUS_READY
while :;do
 if ! pidof bluetoothd >/dev/null 2>&1 && [ "$attempts" -lt 3 ];then
  attempts=$((attempts+1))
  echo "AA_BLUEZ_START: daemon absent; attempt=$attempts"
  /etc/init.d/bluetooth start || echo AA_BLUEZ_START_FAILED
  sleep 3
 fi
 echo 'AA_RADIO_START: independent wireless receiver'
 "$work/catplay-aa-rx" "$work/state" "$SSID" "$AA_VIDEO_SOCKET" >> "$work/carplay.log" 2>&1
 code=$?
 echo "AA_RADIO_EXIT: code=$code; retrying"
 sleep 3
done
