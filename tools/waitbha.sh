#!/bin/bash
F="/mnt/c/Users/xavie/AppData/Local/Temp/claude/C--Users-xavie-Desktop-Exo-OS/a9d46684-607e-4afa-a39d-9299bf8585b1/tasks/bha7g02zy.output"
n=0
while [ "$n" -lt 175 ]; do
  if grep -qaE "log size|BUILDFAIL|error\[" "$F" 2>/dev/null; then break; fi
  sleep 3
  n=$((n + 1))
done
if grep -qaE "BUILDFAIL|error\[" "$F" 2>/dev/null; then
  echo "BUILD FAIL"; grep -aE "error\[|error:" "$F" | head -10; exit 1
fi
L=/mnt/c/Users/xavie/Desktop/Exo-OS/tools/e9d25b.txt
C() { tr -cd '\11\12\15\40-\176' < "$L"; }
echo "=== séquence SW + pr autour du hang (init=p1 cesse-t-il d etre schedulé ?) ==="
C | grep -aoE 'init: pr[0-9]|<CGw>|<SW [0-9]>[0-9]>' | tail -30
echo "=== derniers SW (qui tourne après le hang ?) ==="
C | grep -aoE '<SW [0-9]>[0-9]>' | tail -12
echo "log: $(wc -c < "$L")"
