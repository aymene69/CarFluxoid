#!/bin/sh
# Run as root from the uploaded staging directory. Does not change Wi-Fi.
set -eu
[ "$(uname -m)" = aarch64 ]
[ "$(id -u)" = 0 ]
cd "$(dirname "$0")"
install -d -m 755 /opt/carfluxoid/bin /opt/carfluxoid/assets
install -d -m 700 /opt/carfluxoid/tls /var/lib/carfluxoid/settings /var/lib/carfluxoid/pairing
install -m 755 aa-phone catplay-aa-rx /opt/carfluxoid/bin/
install -m 644 assets/* /opt/carfluxoid/assets/
install -m 600 test-cert.pem test-key.pem /opt/carfluxoid/tls/
install -d /lib/modules/"$(uname -r)"/extra
install -m 644 kernel/aa_bulk.ko /lib/modules/"$(uname -r)"/extra/
depmod -a
install -m 644 carfluxoid-aa.service /etc/systemd/system/
install -d /etc/systemd/journald.conf.d
printf '[Journal]\nStorage=volatile\nRuntimeMaxUse=8M\n' > /etc/systemd/journald.conf.d/carfluxoid.conf
config=/boot/firmware/config.txt
[ -f "$config.before-carfluxoid" ] || cp -p "$config" "$config.before-carfluxoid"
if ! grep -qx 'dtoverlay=dwc2,dr_mode=peripheral' "$config";then
 printf '\n[all]\n# CarFluxoid USB device controller\ndtoverlay=dwc2,dr_mode=peripheral\n' >> "$config"
fi
systemctl daemon-reload
systemctl enable carfluxoid-aa.service
/opt/carfluxoid/bin/catplay-aa-rx --self-test
printf 'Installed ARM64 base. Reboot required for DWC2; Wi-Fi unchanged.\n'
