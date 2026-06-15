#!/bin/bash
F="/mnt/c/Users/xavie/AppData/Local/Temp/claude/C--Users-xavie-Desktop-Exo-OS/a9d46684-607e-4afa-a39d-9299bf8585b1/tasks/bmme1d0ev.output"
n=0
while [ "$n" -lt 175 ]; do
  if grep -qaE "log size|BUILDFAIL|error\[" "$F" 2>/dev/null; then break; fi
  sleep 3
  n=$((n + 1))
done
if grep -qaE "BUILDFAIL|error\[" "$F" 2>/dev/null; then
  echo "=== BUILD FAILED ==="; grep -aE "error\[|error:" "$F" | head -12; exit 1
fi
L=/mnt/c/Users/xavie/Desktop/Exo-OS/tools/e9d25b.txt
C() { tr -cd '\11\12\15\40-\176' < "$L"; }
echo "=== wave-done + rdy (readiness checks) ==="
C | grep -aoE 'init: wave-done, poll|init: rdy [a-z_]+ [a-zA-Z,+]+' | head -16
echo "=== console fin ==="
C | grep -aoE 'init[_a-z]*: [a-zA-Z_/, =0-9-]+|[a-z_]+_server: [a-zA-Z ]+|exosh[^<]*' | tail -10
echo "=== log size ==="; wc -c "$L"
