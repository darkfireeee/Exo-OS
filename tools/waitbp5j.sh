#!/bin/bash
F="/mnt/c/Users/xavie/AppData/Local/Temp/claude/C--Users-xavie-Desktop-Exo-OS/a9d46684-607e-4afa-a39d-9299bf8585b1/tasks/bp5jn9qif.output"
n=0
while [ "$n" -lt 175 ]; do
  if grep -qaE "log size|BUILDFAIL|error\[" "$F" 2>/dev/null; then break; fi
  sleep 3
  n=$((n + 1))
done
L=/mnt/c/Users/xavie/Desktop/Exo-OS/tools/e9d25b.txt
C() { tr -cd '\11\12\15\40-\176' < "$L"; }
echo "=== DMAZERO (frame DMA == pile init = corruption DMA CONFIRMÉE #24/#26) ==="
C | grep -aoE '<DMAZERO f=[0-9a-f]+>' | sort | uniq -c | head
echo "total DMAZERO=$(C | grep -aoc '<DMAZERO') STKFREE=$(C | grep -aoc '<STKFREE') STKDBLMAP=$(C | grep -aoc '<STKDBLMAP') DBLALLOC=$(C | grep -aoc '<DBLALLOC')"
echo "=== STK dump (spf) ==="
C | grep -aoE '<STK rsp=[0-9a-f]+ rbp=[0-9a-f]+ spf=[0-9a-fUNMAPED-]+' | head -2
echo "=== ordre : DMAZERO/STKFREE vs FREEAS/spawnret/SEGV ==="
C | grep -aoE '<DMAZERO f=[0-9a-f]+>|<STKFREE f=[0-9a-f]+>|<FREEAS root=[0-9a-f]+>|init: spawnret [a-z_]+ pid=[0-9]+|<SEGV>' | tail -12
echo "=== build tail ==="
grep -aE "log size|BUILDFAIL|error\[" "$F" | tail -3
