#!/bin/bash
F="/mnt/c/Users/xavie/AppData/Local/Temp/claude/C--Users-xavie-Desktop-Exo-OS/a9d46684-607e-4afa-a39d-9299bf8585b1/tasks/buvnmnxg0.output"
n=0
while [ "$n" -lt 80 ]; do
  if grep -qaE "log size" "$F" 2>/dev/null; then break; fi
  sleep 3
  n=$((n + 1))
done
L=/mnt/c/Users/xavie/Desktop/Exo-OS/tools/e9d180.txt
C() { tr -cd '\11\12\15\40-\176' < "$L"; }
echo "=== RIPCORR (RIP corrompu par context switch) ==="
C | grep -aoE '<RIPCORR before=[0-9a-f]+ after=[0-9a-f]+>' | head -8
echo "total RIPCORR=$(C | grep -aoc '<RIPCORR')"
echo "=== crash atteint ? ==="
C | grep -aoE 'init: spawnret [a-z_]+ pid=[0-9]+|init: pr[0-9]-[a-z-]+|<SEGV>|<RIPCORR[^>]*>' | tail -8
echo "log size: $(wc -c < "$L")"
