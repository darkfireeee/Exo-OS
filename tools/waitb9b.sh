#!/bin/bash
F="/mnt/c/Users/xavie/AppData/Local/Temp/claude/C--Users-xavie-Desktop-Exo-OS/a9d46684-607e-4afa-a39d-9299bf8585b1/tasks/b9b3vdi5r.output"
n=0
while [ "$n" -lt 175 ]; do
  if grep -qaE "log size|BUILDFAIL|error\[" "$F" 2>/dev/null; then break; fi
  sleep 3
  n=$((n + 1))
done
L=/mnt/c/Users/xavie/Desktop/Exo-OS/tools/e9d25b.txt
C() { tr -cd '\11\12\15\40-\176' < "$L"; }
echo "=== séquence pr1-pr5 (dernier = statement qui meurt) ==="
C | grep -aoE 'init: spawnret [a-z_]+ pid=[0-9]+|init: pr[0-9]-[a-z-]+|init: cwave [a-z_]+' | tail -15
echo "=== SEGV/null ==="
echo "SEGV=$(C | grep -aoc '<SEGV') nullX=$(C | grep -aoEc '<PF p1 a=0+ X>')"
echo "=== build tail ==="
grep -aE "log size|BUILDFAIL|error\[" "$F" | tail -3
