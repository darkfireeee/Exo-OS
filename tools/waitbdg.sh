#!/bin/bash
F="/mnt/c/Users/xavie/AppData/Local/Temp/claude/C--Users-xavie-Desktop-Exo-OS/a9d46684-607e-4afa-a39d-9299bf8585b1/tasks/bdg0zvtn0.output"
n=0
while [ "$n" -lt 175 ]; do
  if grep -qaE "log size|BUILDFAIL" "$F" 2>/dev/null; then break; fi
  sleep 3
  n=$((n + 1))
done
L=/mnt/c/Users/xavie/Desktop/Exo-OS/tools/e9d25b.txt
echo "=== FK markers (entrée/sortie fork: parent cr3 + hardware cr3) ==="
tr -cd '\11\12\15\40-\176' < "$L" | grep -aoE '<FK[0E] [a-z_0-9]+=[0-9a-f]+>' | head -16
echo "=== EXEC + console ==="
tr -cd '\11\12\15\40-\176' < "$L" | grep -aoE '<EXEC [^>]*>|init: [a-z_]+ [a-z_0-9=]+' | head -10
