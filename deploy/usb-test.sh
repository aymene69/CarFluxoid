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
[ -n "$udc" ]
iphone=
for d in /sys/class/iphone/*;do
 [ -f "$d/udc" ] && [ "$(cat "$d/udc")" = "$udc" ] && iphone=$d
done
[ -n "$iphone" ]
stage=preparer_reception
./catplay-aa-rx --self-test
mkdir "$base/state"
cp /persist/certificate.p7b /persist/identity.pk8 "$base/state/"
[ ! -f /persist/homekit_rx_db.bin ] || cp /persist/homekit_rx_db.bin "$base/state/"
chmod 700 "$base/state"
chmod 600 "$base/state/"*
. "$base/wifi-env.sh"
export AA_VIDEO_SOCKET="$base/video.sock"
stage=liberer_iphone
echo AA_FLOW_STOCK_CATPLAY_DISABLED
:
sleep 1
echo 0 > "$iphone/bind"
stage=charger_module_queue
insmod "$base/aa_bulk.ko"
loaded=1
[ -c /dev/aa_bulk ]
echo 'AA_USB_BOUND : pilote AA noyau actif. DHU Google attendu. Huit requêtes RX préallouées.'
stage=session_android_auto
export AA_USB_FAST_READ=1
export AA_AUTONOMOUS=1
export AA_WAIT_SCREEN_DIR="$base"
./aa-phone-v821 --raw /dev/aa_bulk "$base/test-cert.pem" "$base/test-key.pem" "$base/mire.h264" > "$base/phone.log" 2>&1 &
phone_pid=$!
tries=0
until [ -s "$AA_VIDEO_SOCKET.display" ]; do
 kill -0 "$phone_pid" || { cat "$base/phone.log";exit 1; }
 tries=$((tries+1)); [ "$tries" -lt 45 ] || { echo 'ERREUR : résolution DHU absente';exit 1; }
 sleep 1
done
echo "AA_FLOW : DHU $(cat "$AA_VIDEO_SOCKET.display"), lancement CarPlay."
./catplay-aa-rx "$base/state" "$SSID" "$AA_VIDEO_SOCKET" > "$base/carplay.log" 2>&1 &
rx_pid=$!
echo 'AA_FLOW : connecter iPhone à CarFluxoid et accepter CarPlay.'
while kill -0 "$phone_pid" 2>/dev/null; do
 kill -0 "$rx_pid" || { cat "$base/carplay.log";exit 1; }
 sleep 1
done
wait "$phone_pid"
phone_pid=
cat "$base/phone.log"
