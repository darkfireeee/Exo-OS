#!/bin/bash
F="/mnt/c/Users/xavie/AppData/Local/Temp/claude/C--Users-xavie-Desktop-Exo-OS/a9d46684-607e-4afa-a39d-9299bf8585b1/tasks/b7v2jmhux.output"
n=0
while [ "$n" -lt 175 ]; do
  if grep -qaE "log size|BUILDFAIL" "$F" 2>/dev/null; then break; fi
  sleep 3
  n=$((n + 1))
done
echo "=== FK markers (parent cr3 entrée/sortie du fork) ==="
tr -cd '\11\12\15\40-\176' < /mnt/c/Users/xavie/Desktop/Exo-OS/tools/e9d25b.txt | grep -aoE '<FK[0E] [a-z_0-9]+=[0-9a-f]+>' | head -20
echo "=== EXEC ==="
tr -cd '\11\12\15\40-\176' < /mnt/c/Users/xavie/Desktop/Exo-OS/tools/e9d25b.txt | grep -aoE '<EXEC [^>]*>' | sort | uniq -c
echo "=== console ==="
tr -cd '\11\12\15\40-\176' < /mnt/c/Users/xavie/Desktop/Exo-OS/tools/e9d25b.txt | grep -aoE 'init[_a-z]*: [a-zA-Z_/, =0-9-]+' | head -12
