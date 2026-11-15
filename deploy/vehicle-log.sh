#!/bin/sh
# Two bounded boot records, no certificates/passwords and no continuous flash writes.
dir=/persist/aa-diagnostics
mkdir -p "$dir" || exit 0
chmod 700 "$dir"
[ ! -f "$dir/current.log" ] || mv -f "$dir/current.log" "$dir/previous.log"
record(){
 (umask 077
 { date -u; cat /proc/uptime; cat /proc/cmdline
   for n in /sys/class/udc/*;do echo "UDC=$n";cat "$n/state" "$n/function" 2>/dev/null;done
   for n in /sys/class/usb_role/*/role;do [ ! -f "$n" ] || { echo "$n";cat "$n"; };done
   cat /proc/meminfo | head -n 8
   echo CLOCK;cat /persist/aa-clock-epoch /tmp/aa-clock-status 2>/dev/null
   echo BOOT;tail -c 12000 /var/log/aa-bridge-boot.log 2>/dev/null
   echo PHONE;tail -c 12000 /tmp/aa-autoboot/phone.log 2>/dev/null
   echo RADIO;tail -c 6000 /tmp/aa-autoboot/carplay.log 2>/dev/null
   echo KERNEL;dmesg | tail -n 60
 } > "$dir/current.log.new"
 mv -f "$dir/current.log.new" "$dir/current.log";sync)
}
# Capture early failure, USB negotiation and later session; then stop writing.
for delay in 0 15 30 60 180;do sleep "$delay";record;done
