#!/bin/bash
F="/mnt/c/Users/xavie/AppData/Local/Temp/claude/C--Users-xavie-Desktop-Exo-OS/a9d46684-607e-4afa-a39d-9299bf8585b1/tasks/bh4sbsmsu.output"
n=0
while [ "$n" -lt 175 ]; do
  if grep -qaE "log size|BUILDFAIL|error\[" "$F" 2>/dev/null; then break; fi
  sleep 3
  n=$((n + 1))
done
L=/mnt/c/Users/xavie/Desktop/Exo-OS/tools/e9d25b.txt
C() { tr -cd '\11\12\15\40-\176' < "$L"; }
echo "=== MVLEAF (move_leaf vers/depuis la pile = mremap écrase la pile) ==="
C | grep -aoE '<MVLEAF src=[0-9a-f]+ dst=[0-9a-f]+>' | sort | uniq -c | head -15
echo "total MVLEAF=$(C | grep -aoc '<MVLEAF')"
echo "=== STK dump (spf) + SEGV ==="
C | grep -aoE '<STK rsp=[0-9a-f]+ rbp=[0-9a-f]+ spf=[0-9a-fUNMAPED-]+|<MVLEAF[^>]*>|init: spawnret [a-z_]+ pid=[0-9]+|<SEGV>' | tail -12
echo "=== build tail ==="
grep -aE "log size|BUILDFAIL|error\[" "$F" | tail -3
