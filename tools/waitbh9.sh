#!/bin/bash
F="/mnt/c/Users/xavie/AppData/Local/Temp/claude/C--Users-xavie-Desktop-Exo-OS/a9d46684-607e-4afa-a39d-9299bf8585b1/tasks/bh9rw6k8q.output"
n=0
while [ "$n" -lt 175 ]; do
  if grep -qaE "log size|BUILDFAIL|error\[" "$F" 2>/dev/null; then break; fi
  sleep 3
  n=$((n + 1))
done
L=/mnt/c/Users/xavie/Desktop/Exo-OS/tools/e9d25b.txt
C() { tr -cd '\11\12\15\40-\176' < "$L"; }
echo "=== STKFREE (frame pile libéré par le buddy = use-after-free CONFIRMÉ) ==="
C | grep -aoE '<STKFREE f=[0-9a-f]+>' | sort | uniq -c | head -20
echo "total STKFREE: $(C | grep -aoc "<STKFREE")"
echo "=== séquence STKFREE vs spawn/segv (ordre chrono) ==="
C | grep -aoE '<STKFREE f=[0-9a-f]+>|init: spawnret [a-z_]+ pid=[0-9]+|init: cwave [a-z_]+|<SEGV>|<PF p1 a=0+ X>|<FREEAS root=[0-9a-f]+>' | tail -20
echo "=== build tail ==="
grep -aE "log size|BUILDFAIL|error\[" "$F" | tail -3
