#!/bin/bash
F="/mnt/c/Users/xavie/AppData/Local/Temp/claude/C--Users-xavie-Desktop-Exo-OS/a9d46684-607e-4afa-a39d-9299bf8585b1/tasks/b0n3z2i11.output"
n=0
while [ "$n" -lt 160 ]; do
  if grep -qaE 'log size|BUILDFAIL' "$F" 2>/dev/null; then break; fi
  sleep 3
  n=$((n + 1))
done
echo "=== RESULT (n=$n) ==="
cat "$F"
