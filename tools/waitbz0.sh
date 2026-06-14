#!/bin/bash
F="/mnt/c/Users/xavie/AppData/Local/Temp/claude/C--Users-xavie-Desktop-Exo-OS/a9d46684-607e-4afa-a39d-9299bf8585b1/tasks/bz0hxyhwt.output"
n=0
while [ "$n" -lt 175 ]; do
  if grep -qaE "log size|BUILDFAIL" "$F" 2>/dev/null; then break; fi
  sleep 3
  n=$((n + 1))
done
L=/mnt/c/Users/xavie/Desktop/Exo-OS/tools/e9d25b.txt
echo "=== CONSOLE (init/serveurs/shell) ==="
tr -cd '\11\12\15\40-\176' < "$L" | grep -aoE 'init[_a-z]*: [a-zA-Z_/, =0-9-]+|[a-z_]+_server: [a-zA-Z ]+|exosh[^<]+|init: chk [a-z_]+|init: ready [a-z_]+' | head -50
echo "=== EXEC serveurs ==="
tr -cd '\11\12\15\40-\176' < "$L" | grep -aoE '<EXEC [^>]*>' | sort | uniq -c
echo "=== K0/LKP (readiness atteinte ?) ==="
tr -cd '\11\12\15\40-\176' < "$L" | grep -aoE '<K0 [^>]*>|<LKP [^>]*>' | sort | uniq -c | head
echo "=== CR3 faults (mismatch résiduel ?) ==="
tr -cd '\11\12\15\40-\176' < "$L" | grep -aoE '<CR3 [^>]*>' | sort | uniq -c | head -6
echo "=== log size ==="; wc -c "$L"
