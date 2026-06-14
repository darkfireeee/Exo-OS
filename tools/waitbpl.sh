#!/bin/bash
F="/mnt/c/Users/xavie/AppData/Local/Temp/claude/C--Users-xavie-Desktop-Exo-OS/a9d46684-607e-4afa-a39d-9299bf8585b1/tasks/bplnzje81.output"
n=0
while [ "$n" -lt 175 ]; do
  if grep -qaE "log size|BUILDFAIL|error\[" "$F" 2>/dev/null; then break; fi
  sleep 3
  n=$((n + 1))
done
L=/mnt/c/Users/xavie/Desktop/Exo-OS/tools/e9d25b.txt
C() { tr -cd '\11\12\15\40-\176' < "$L"; }
echo "=== séquence autour de memory_server (cwave/cs/md*) ==="
C | grep -aoE 'init: cwave [a-z_]+|init: cs[0-9]|init: md[INLFX]|init: canstart [a-z_]+' | tail -25
echo "=== SEGV / null ==="
C | grep -aoE '<SEGV>|<PF p1 a=0+ X>' | sort | uniq -c
echo "=== build tail ==="
grep -aE "log size|BUILDFAIL|error\[" "$F" | tail -3
