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
while :;do
 echo 'AA_RADIO_START: independent wireless receiver'
 "$work/catplay-aa-rx" "$work/state" "$SSID" "$AA_VIDEO_SOCKET" >> "$work/carplay.log" 2>&1
 code=$?
 echo "AA_RADIO_EXIT: code=$code; retrying"
 sleep 3
done
