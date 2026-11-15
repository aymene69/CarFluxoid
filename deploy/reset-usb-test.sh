#!/bin/sh
# RAM test only. Release the installed sender before replacing its module.
set -eu
sys=${AA_RESET_SYS:-/sys}
proc=${AA_RESET_PROC:-/proc}
# Prevent an installed supervisor from respawning the sender during module removal.
for p in "$proc"/[0-9]*/cmdline;do
 [ -f "$p" ] || continue
 c=$(tr '\000' ' ' < "$p" 2>/dev/null || true)
 case "$c" in
 '/bin/sh /tmp/aa-autoboot/usb-test.sh'*|'sh /tmp/aa-autoboot/usb-test.sh'*|'/bin/sh /tmp/aa-vehicle-'*|'sh /tmp/aa-vehicle-'*|'/bin/sh /usr/lib/aa-bridge/radio-service.sh'*|'/bin/sh /usr/lib/aa-bridge/clock-service.sh'*)
  pid=${p%/cmdline};pid=${pid##*/}
  [ "$pid" != "$$" ] || continue
  kill -STOP "$pid" 2>/dev/null || true
  kill -KILL "$pid" 2>/dev/null || true
  ;;
 esac
done
# Cancel queued USB transfers; a writer may otherwise wait forever for the host.
if [ -d "$sys/module/aa_bulk" ];then
 for udc in "$sys"/class/udc/*;do
  if [ -w "$udc/soft_connect" ];then
   if echo disconnect > "$udc/soft_connect";then echo 'AA_RESET: USB soft-disconnected';else echo 'AA_RESET: soft disconnect unavailable';fi
  fi
 done
fi
killall aa-phone-v821 catplay-aa-rx 2>/dev/null || true
sleep 1
killall -9 aa-phone-v821 catplay-aa-rx 2>/dev/null || true
tries=0
while [ -d "$sys/module/aa_bulk" ] && [ "$tries" -lt 5 ];do
 if rmmod aa_bulk;then break;fi
 tries=$((tries+1));sleep 1
done
if [ -d "$sys/module/aa_bulk" ];then
 echo 'AA_RESET_FAILED: old aa_bulk still loaded; new test was NOT started'
 cat "$sys/module/aa_bulk/refcnt" 2>/dev/null || true
 cat "$proc/modules" 2>/dev/null || true
 exit 1
fi
for i in "$sys"/class/iphone/*;do
 [ ! -w "$i/bind" ] || echo 1 > "$i/bind"
done
rmdir "${AA_RESET_LOCK:-/tmp/aa-lab-lock}" 2>/dev/null || true
echo 'AA_RESET_OK: old module absent; stock device presentation restored'
