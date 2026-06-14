#!/bin/bash
F="/mnt/c/Users/xavie/AppData/Local/Temp/claude/C--Users-xavie-Desktop-Exo-OS/a9d46684-607e-4afa-a39d-9299bf8585b1/tasks/bjer2don6.output"
n=0
while [ "$n" -lt 175 ]; do
  if grep -qaE "log size|BUILDFAIL|error\[" "$F" 2>/dev/null; then break; fi
  sleep 3
  n=$((n + 1))
done
L=/mnt/c/Users/xavie/Desktop/Exo-OS/tools/e9d25b.txt
C() { tr -cd '\11\12\15\40-\176' < "$L"; }
echo "=== DBLALLOC (frame réalloué sans free = double-alloc CONFIRMÉ) ==="
C | grep -aoE '<DBLALLOC [0-9a-f]+>' | sort | uniq -c | head -20
echo "total DBLALLOC: $(C | grep -aoc "<DBLALLOC")"
echo "=== séquence : dernier DBLALLOC vs fork/spawn/SEGV (ordre) ==="
C | grep -aoE '<DBLALLOC [0-9a-f]+>|init: spawnret [a-z_]+ pid=[0-9]+|init: cwave [a-z_]+|<SEGV>|<PF p1 a=0+ X>|<FK0[^>]*>|<FKE[^>]*>' | tail -25
echo "=== build tail ==="
grep -aE "log size|BUILDFAIL|error\[" "$F" | tail -3
