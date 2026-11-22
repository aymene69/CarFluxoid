#!/bin/sh
# Run only on the Pi, from the verified ARM64 release directory.
set -eu
[ "$(id -u)" -eq 0 ]
[ "$(uname -m)" = aarch64 ]
cd "$(dirname "$0")"
sha256sum -c SHA256SUMS
if ! /usr/bin/python3 -c 'from PIL import Image' || ! command -v ffmpeg >/dev/null || ! test -r /usr/share/fonts/truetype/dejavu/DejaVuSans.ttf;then
 apt-get update
 apt-get install -y --no-install-recommends python3-pil ffmpeg fonts-dejavu-core
fi
./catplay-aa-rx --self-test
umask 077
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT HUP INT TERM
mkdir "$scratch/run"
cp waiting_iphone_config.py "$scratch/"
ln -s /opt/carfluxoid/assets/attente-800x480.h264 "$scratch/attente-800x480.h264"
PYTHONDONTWRITEBYTECODE=1 PYTHONPATH="$PWD" /usr/bin/python3 - "$scratch" <<'PY'
import sys
from pathlib import Path
from dashboard import Dashboard

scratch = Path(sys.argv[1])
Dashboard(Path("/var/lib/carfluxoid/settings"), scratch, scratch/"run", scratch).publish()
PY

backup=$(mktemp -d /opt/carfluxoid/dashboard-backup.XXXXXX)
cp -p /opt/carfluxoid/bin/aa-phone /opt/carfluxoid/bin/catplay-aa-rx /opt/carfluxoid/bin/start-ap-trial.sh "$backup/"
cp -p /etc/systemd/system/carfluxoid-radio.service "$backup/"
for path in /opt/carfluxoid/bin/dashboard.py /opt/carfluxoid/bin/start-radio.sh \
 /opt/carfluxoid/assets/waiting_iphone_config.py /etc/systemd/system/carfluxoid-dashboard.service;do
 if [ -f "$path" ];then cp -p "$path" "$backup/";fi
done
install -d -m 755 /etc/systemd/system/carfluxoid-aa.service.d /etc/systemd/system/carfluxoid-radio.service.d
if [ -f /etc/systemd/system/carfluxoid-aa.service.d/dashboard.conf ];then
 cp -p /etc/systemd/system/carfluxoid-aa.service.d/dashboard.conf "$backup/aa-dashboard.conf"
fi
if [ -f /etc/systemd/system/carfluxoid-radio.service.d/dashboard.conf ];then
 cp -p /etc/systemd/system/carfluxoid-radio.service.d/dashboard.conf "$backup/radio-dashboard.conf"
fi
for binary in aa-phone catplay-aa-rx;do
 install -m 755 "$binary" "/opt/carfluxoid/bin/$binary.new"
 mv "/opt/carfluxoid/bin/$binary.new" "/opt/carfluxoid/bin/$binary"
done
install -m 755 start-radio.sh start-ap-trial.sh dashboard.py car-diag.sh /opt/carfluxoid/bin/
systemctl stop carfluxoid-home-return.timer carfluxoid-home-return.service 2>/dev/null || true
install -m 644 waiting_iphone_config.py /opt/carfluxoid/assets/
install -m 644 carfluxoid-dashboard.service carfluxoid-car-diag.service /etc/systemd/system/
printf '[Unit]\nStartLimitIntervalSec=0\n\n[Service]\nRuntimeDirectoryPreserve=yes\nRestartSec=0.2\nEnvironment=AA_DASHBOARD_DIR=/run/carfluxoid-ui\nEnvironment=AA_AUDIO_PREROLL=450\n' > /etc/systemd/system/carfluxoid-aa.service.d/dashboard.conf
printf '[Service]\nExecStart=\nExecStart=/opt/carfluxoid/bin/start-radio.sh\n' > /etc/systemd/system/carfluxoid-radio.service.d/dashboard.conf
rm -f /etc/modprobe.d/carfluxoid-aa.conf
chmod 644 /etc/systemd/system/carfluxoid-aa.service.d/dashboard.conf /etc/systemd/system/carfluxoid-radio.service.d/dashboard.conf
systemctl daemon-reload
systemctl restart carfluxoid-aa carfluxoid-web
systemctl enable carfluxoid-dashboard carfluxoid-car-diag
systemctl restart carfluxoid-dashboard carfluxoid-car-diag
systemctl try-restart carfluxoid-radio
dashboard_ready=no
for attempt in 1 2 3 4 5 6 7 8 9 10;do
 for menu in /run/carfluxoid-ui/dashboard-[0-9]*.h264;do
  size=${menu##*/dashboard-}
  if [ -s "$menu" ] && [ -s "/run/carfluxoid-ui/dashboard-wait-$size" ];then
   dashboard_ready=yes
   break 2
  fi
 done
 sleep 1
done
if [ "$dashboard_ready" = no ];then
 echo "Le tableau de bord n’a pas publié le menu et l’écran d’attente. Sauvegarde : $backup"
 journalctl -u carfluxoid-dashboard -n 20 --no-pager
 exit 1
fi
for service in carfluxoid-aa carfluxoid-web carfluxoid-dashboard;do
 systemctl is-active --quiet "$service" || {
  echo "Service indisponible : $service. Sauvegarde : $backup"
  journalctl -u "$service" -n 20 --no-pager
  exit 1
 }
done
printf 'Tableau de bord installé. Sauvegarde : %s\n' "$backup"
