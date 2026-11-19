#!/bin/sh
# Configure the profile but deliberately keep the current Wi-Fi connection.
set -eu
[ "$(id -u)" = 0 ]
cd "$(dirname "$0")"
install -d -m 700 /var/lib/carfluxoid/identity /etc/carfluxoid
for name in certificate.p7b identity.pk8;do
 [ -s "$name" ] || { echo "Missing private identity: $name"; exit 1; }
 install -m 600 "$name" /var/lib/carfluxoid/identity/
done
install -m 644 carfluxoid-radio.service carfluxoid-web.service /etc/systemd/system/
umask 077
if [ ! -f /etc/carfluxoid/radio.env ];then
 password=$(od -An -N 12 -tx1 /dev/urandom | tr -d ' \n')
 [ "${#password}" -eq 24 ]
 printf 'AA_WIFI_SSID=CarFluxoid-Pi_AP\nAA_WIFI_PASSWORD=%s\n' "$password" > /etc/carfluxoid/radio.env
fi
if ! nmcli -g connection.id connection show carfluxoid-ap >/dev/null 2>&1;then
 password=$(sed -n 's/^AA_WIFI_PASSWORD=//p' /etc/carfluxoid/radio.env)
 [ "${#password}" -ge 8 ] && [ "${#password}" -le 63 ] || {
  echo 'Set a Wi-Fi passphrase of 8 to 63 characters in /etc/carfluxoid/radio.env' >&2
  exit 1
 }
 nmcli connection add type wifi ifname wlan0 con-name carfluxoid-ap autoconnect no ssid CarFluxoid-Pi_AP
 nmcli connection modify carfluxoid-ap 802-11-wireless.mode ap 802-11-wireless.band bg 802-11-wireless.channel 6 \
  802-11-wireless.powersave 2 802-11-wireless-security.key-mgmt wpa-psk 802-11-wireless-security.psk "$password" \
  ipv4.method shared ipv4.addresses 192.168.50.2/24 ipv6.method link-local ipv6.addr-gen-mode eui64
fi
systemctl daemon-reload
systemctl enable --now carfluxoid-web.service
printf 'Radio identity and AP profile ready. Home Wi-Fi unchanged; radio not started.\n'
