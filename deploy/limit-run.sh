#!/bin/sh
# Portable RAM-test deadline; no external timeout utility required.
set -u
seconds=$1;shift
case "$seconds" in ''|*[!0-9]*) exit 2;; esac
[ "$seconds" -gt 0 ] && [ "$seconds" -le 3600 ] || exit 2
"$@" &
child=$!
cleanup(){ kill -TERM "$child" 2>/dev/null || true; }
trap 'cleanup; exit 143' TERM INT
count=0
while kill -0 "$child" 2>/dev/null;do
 if [ "$count" -ge "$seconds" ];then
  echo 'AA_RAM_DEADLINE: stopping temporary test'
  cleanup
  wait "$child" 2>/dev/null || true
  exit 124
 fi
 sleep 1
 count=$((count+1))
done
code=0;wait "$child" || code=$?
exit "$code"
