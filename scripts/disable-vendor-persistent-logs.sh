#!/bin/sh
# One explicit configuration change, no firmware/identity/pairing modification.
set -eu
config=${AA_VENDOR_CONFIG:-/persist/config.txt}
[ ! -L "$config" ] || { echo 'Refusing a symlinked vendor config';exit 1; }
umask 077
candidate="$config.logging.new"
trap 'rm -f "$candidate"' EXIT INT TERM
if [ -f "$config" ];then
 awk 'BEGIN{done=0} /^[[:space:]]*log_persist[[:space:]]*=/{if(!done)print "log_persist=0";done=1;next} {print} END{if(!done)print "log_persist=0"}' "$config" > "$candidate"
 if cmp -s "$config" "$candidate";then echo 'Vendor persistent logging already disabled; unchanged.';exit 0;fi
 [ -e "$config.before-lowwrite" ] || cp "$config" "$config.before-lowwrite"
else printf '%s\n' 'log_persist=0' > "$candidate";fi
sync
mv -f "$candidate" "$config"
sync
echo 'Vendor log_persist=0 saved; applies on next boot. Other settings preserved.'
