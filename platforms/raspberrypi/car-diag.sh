#!/bin/sh
# Persists bounded CarFluxoid diagnostics to SD card so vehicle logs survive power-off.
set -u
DIR=/var/lib/carfluxoid
LOG="$DIR/car-diag.log"
PREV="$DIR/car-diag.prev.log"
TMP="$DIR/.car-diag.tmp"

mkdir -p "$DIR"
if [ -s "$LOG" ]; then
 mv -f "$LOG" "$PREV"
 sync
fi

snapshot() {
 label=$1
 {
  echo "=== SNAPSHOT $label ($(cat /proc/uptime 2>/dev/null || echo ?)) ==="
  vcgencmd get_throttled 2>/dev/null || true
  vcgencmd measure_temp 2>/dev/null || true
  echo "--- aa_bulk ---"
  grep . /sys/module/aa_bulk/parameters/* 2>/dev/null || echo aa_bulk_absent
  echo "--- dmesg (head) ---"
  dmesg 2>/dev/null | grep -iE 'aa_bulk|dwc2|under-voltage|voltage|brcmfmac' | head -n 30 || true
  echo "--- dmesg (tail) ---"
  dmesg 2>/dev/null | grep -iE 'aa_bulk|dwc2|under-voltage|voltage|brcmfmac' | tail -n 40 || true
  echo "--- carfluxoid-dashboard ---"
  journalctl -u carfluxoid-dashboard -n 30 --no-pager 2>/dev/null || true
  echo "--- carfluxoid-aa ---"
  journalctl -u carfluxoid-aa -n 80 --no-pager 2>/dev/null || true
  echo "--- carfluxoid-radio (key events) ---"
  journalctl -u carfluxoid-radio --no-pager 2>/dev/null | grep -E 'CP_|BT_QUIET|AUDIO_|VIDEO_' | tail -n 80 || true
  echo "--- carfluxoid-radio (tail) ---"
  journalctl -u carfluxoid-radio -n 40 --no-pager 2>/dev/null || true
  echo "--- session.log ---"
  if [ -r /run/carfluxoid/session.log ]; then
   lines=$(wc -l < /run/carfluxoid/session.log)
   if [ "$lines" -le 400 ]; then
    cat /run/carfluxoid/session.log
   else
    head -n 120 /run/carfluxoid/session.log
    echo "... ($lines lines total, showing tail 280) ..."
    tail -n 280 /run/carfluxoid/session.log
   fi
  else
   echo no_session_log
  fi
 } > "$TMP" 2>&1
 mv -f "$TMP" "$LOG"
 sync
}

for delay in 20 40 60 120; do
 sleep "$delay"
 snapshot "+${delay}s"
done
