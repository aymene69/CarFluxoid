#!/bin/sh
set -eu
cd "$(dirname "$0")"
base=$PWD
stage=verification
loaded=0
phone_pid=
rx_pid=
cleanup(){
 code=$?
 trap - EXIT INT TERM
 [ -z "$rx_pid" ] || kill "$rx_pid" 2>/dev/null || true
 [ -z "$phone_pid" ] || kill "$phone_pid" 2>/dev/null || true
 [ "$code" = 0 ] || echo "ERREUR AA : etape=$stage code=$code"
 if [ "$loaded" = 0 ]; then
  [ -z "${iphone:-}" ] || echo 1 > "$iphone/bind" || true
  :
 fi
 rmdir /tmp/aa-lab-lock 2>/dev/null || true
 echo 'AA_FLOW : débrancher/rebrancher pour revenir à la bêta ; module de test uniquement en RAM.'
 echo AA_LAB_FINISHED
}
[ ! -d /sys/module/aa_bulk ] || { echo 'ERREUR : module de test déjà présent. Débrancher/rebrancher avant cet essai.';exit 1; }
mkdir /tmp/aa-lab-lock
trap cleanup EXIT INT TERM
[ "$(uname -r)" = 7.2.13 ]
[ "$(ls -d /sys/class/udc/* | wc -l)" -eq 1 ] || { echo "ERREUR : contrôleur USB unique requis";exit 1; }
udc=
for c in /sys/class/udc/*; do
 case "$(cat "$c/function" 2>/dev/null || true)" in iphone*|iPhone*) udc=${c##*/};; esac
done
[ -n "$udc" ] || udc=$(basename /sys/class/udc/*)
iphone=
for d in /sys/class/iphone/*;do
 [ -f "$d/udc" ] && [ "$(cat "$d/udc")" = "$udc" ] && iphone=$d
done
[ -n "$iphone" ] || echo "AA_BOOT: no stock iPhone gadget to release"
stage=preparer_reception
./catplay-aa-rx --self-test
if [ "${AA_RADIO_EXTERNAL:-0}" != 1 ];then
mkdir -p "$base/state"
cp /persist/certificate.p7b /persist/identity.pk8 "$base/state/"
[ ! -f /persist/homekit_rx_db.bin ] || cp /persist/homekit_rx_db.bin "$base/state/"
chmod 700 "$base/state"
chmod 600 "$base/state/"*
fi
. "$base/wifi-env.sh"
export AA_VIDEO_SOCKET="$base/video.sock"
stage=liberer_iphone
echo AA_FLOW_STOCK_CATPLAY_DISABLED
:
sleep 1
[ -z "$iphone" ] || echo 0 > "$iphone/bind"
stage=charger_module_queue
usb_mode=${AA_USB_MODE:-negotiated}
if [ -z "${AA_USB_MODE:-}" ] && [ -f /persist/aa-settings/usb-mode ];then usb_mode=$(cat /persist/aa-settings/usb-mode);fi
case "$usb_mode" in direct) pre=0;; negotiated) pre=1;; *) echo 'Invalid USB mode';exit 1;; esac
insmod "$base/aa_bulk.ko" pre_accessory="$pre"
echo "AA_USB_MODE=$usb_mode"
loaded=1
[ -c /dev/aa_bulk ]
echo 'AA_USB_BOUND : pilote AA noyau actif. DHU Google attendu. Huit requêtes RX préallouées.'
stage=session_android_auto
export AA_USB_FAST_READ=1 AA_AUTONOMOUS=1 AA_WAIT_SCREEN_DIR="$base"
# Wireless reception must remain available even if the head unit never starts AA.
if [ "${AA_RADIO_EXTERNAL:-0}" != 1 ];then
 ./catplay-aa-rx "$base/state" "$SSID" "$AA_VIDEO_SOCKET" > "$base/carplay.log" 2>&1 &
 rx_pid=$!
fi
echo 'AA_RADIO_STARTED: CarFluxoid Bluetooth available independently of Android Auto.'
while :; do
 rm -f "$AA_VIDEO_SOCKET.display"
 ./aa-phone-v821 --raw /dev/aa_bulk "$base/test-cert.pem" "$base/test-key.pem" "$base/mire.h264" >> "$base/phone.log" 2>&1 &
 phone_pid=$!
 while kill -0 "$phone_pid" 2>/dev/null; do
  if [ -n "$rx_pid" ] && ! kill -0 "$rx_pid" 2>/dev/null; then
   echo 'AA_RADIO_RESTART: receiver exited; restarting without changing USB.'
   ./catplay-aa-rx "$base/state" "$SSID" "$AA_VIDEO_SOCKET" >> "$base/carplay.log" 2>&1 &
   rx_pid=$!
  fi
  sleep 2
 done
 code=0;wait "$phone_pid" || code=$?
 phone_pid=
 echo "AA_SESSION_RESTART: code=$code; radio remains active"
 sleep 3
done
