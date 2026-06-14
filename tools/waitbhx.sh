#!/bin/bash
F="/mnt/c/Users/xavie/AppData/Local/Temp/claude/C--Users-xavie-Desktop-Exo-OS/a9d46684-607e-4afa-a39d-9299bf8585b1/tasks/bhxo17b5b.output"
n=0
while [ "$n" -lt 175 ]; do
  if grep -qaE "log size|BUILDFAIL|error\[" "$F" 2>/dev/null; then break; fi
  sleep 3
  n=$((n + 1))
done
L=/mnt/c/Users/xavie/Desktop/Exo-OS/tools/e9d25b.txt
C() { tr -cd '\11\12\15\40-\176' < "$L"; }
echo "=== STK dump (spf = frame phys de la pile d init au saut NULL) ==="
C | grep -aoE '<STK rsp=[0-9a-f]+ rbp=[0-9a-f]+ spf=[0-9a-fUNMAPED-]+' | head -3
echo "=== STKFREE ==="
C | grep -aoE '<STKFREE f=[0-9a-f]+>' | sort | uniq -c | head
echo "total STKFREE: $(C | grep -aoc '<STKFREE')"
echo "=== DPM frames init (range) ==="
C | grep -aoE '<DPM P[01] f=[0-9a-f]+>' | head -3
C | grep -aoE '<DPM P[01] f=[0-9a-f]+>' | tail -3
echo "=== FREEAS + contexte ==="
C | grep -aoE '<FREEAS root=[0-9a-f]+>|<SEGV>|init: spawnret [a-z_]+ pid=[0-9]+' | tail -6
echo "=== build tail ==="
grep -aE "log size|BUILDFAIL|error\[" "$F" | tail -3
