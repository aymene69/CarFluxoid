#!/bin/sh
# Independent supervisor: AA session completion must not stop configuration.
work=${AA_WEB_WORK:-/tmp/aa-autoboot}
log=${AA_WEB_LOG:-$work/web.log}
lock=${AA_WEB_LOCK:-/tmp/aa-web-supervisor.lock}
if ! mkdir "$lock" 2>/dev/null; then
 pid=$(cat "$lock/pid" 2>/dev/null || true)
 if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null;then exit 0;fi
 rm -f "$lock/pid";rmdir "$lock" 2>/dev/null || exit 1
 mkdir "$lock" || exit 1
fi
echo $$ > "$lock/pid"
trap 'rm -f "$lock/pid";rmdir "$lock" 2>/dev/null || true' EXIT
while [ -x "$work/catplay-aa-rx" ]; do
 [ "$(wc -c < "$log" 2>/dev/null || echo 0)" -lt 32768 ] || tail -c 16384 "$log" > "$log.old"
 if [ -f "$log.old" ]; then mv "$log.old" "$log";fi
 echo "WEB_SERVICE_START uptime=$(cut -d ' ' -f 1 /proc/uptime)" >> "$log"
 "$work/catplay-aa-rx" --web >> "$log" 2>&1
 code=$?
 echo "WEB_SERVICE_EXIT code=$code uptime=$(cut -d ' ' -f 1 /proc/uptime)" >> "$log"
 awk '/MemAvailable|MemFree/ {print}' /proc/meminfo >> "$log"
 sleep 3
done
