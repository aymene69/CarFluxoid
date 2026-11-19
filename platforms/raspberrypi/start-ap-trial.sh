#!/bin/sh
# Activate the AP. Keep the old script name for existing installations.
set -eu
[ "$(id -u)" = 0 ]
case "${1:-}" in
 '') radio_action=start ;;
 --restart) radio_action=restart ;;
 *) echo 'Usage: start-ap-trial.sh [--restart]' >&2; exit 2 ;;
esac
systemctl stop carfluxoid-home-return.timer carfluxoid-home-return.service 2>/dev/null || true
nmcli radio wifi on
if [ "$(nmcli -g GENERAL.CONNECTION device show wlan0)" = carfluxoid-ap ];then
 systemctl "$radio_action" carfluxoid-radio.service
 echo 'Point d’accès CarFluxoid actif sans minuterie.'
 exit 0
fi
home=$(nmcli -t -f UUID,DEVICE connection show --active | awk -F: '$2=="wlan0"{print $1;exit}')
name_file=/var/lib/carfluxoid/settings/wifi-name.txt
if [ -s "$name_file" ];then
 ssid=$(cat "$name_file")
 if ! printf '%s\n' "$ssid" | LC_ALL=C grep -Eq '^[A-Za-z0-9][A-Za-z0-9 ._-]{0,31}$';then
  echo 'Nom Wi-Fi invalide'
  exit 1
 fi
 nmcli connection modify carfluxoid-ap 802-11-wireless.ssid "$ssid"
fi
if ! nmcli --wait 20 connection up carfluxoid-ap ifname wlan0;then
 if [ -n "$home" ];then nmcli --wait 15 connection up uuid "$home";fi
 exit 1
fi
# Reload a receiver that might already be advertising over Bluetooth from the home Wi-Fi.
systemctl restart carfluxoid-radio.service
echo 'Point d’accès CarFluxoid actif sans minuterie.'
