#!/bin/sh
set -eu
# Only use the AP name that NetworkManager actually activated.
ssid=${AA_WIFI_SSID:-CarFluxoid-Pi_AP}
if [ "$(nmcli -g GENERAL.CONNECTION device show wlan0)" = carfluxoid-ap ];then
 ssid=$(nmcli --escape no -g 802-11-wireless.ssid connection show carfluxoid-ap)
fi
exec /opt/carfluxoid/bin/catplay-aa-rx /var/lib/carfluxoid/identity "$ssid" /run/carfluxoid/video.sock
