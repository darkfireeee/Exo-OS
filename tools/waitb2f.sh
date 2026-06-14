#!/bin/bash
F="/mnt/c/Users/xavie/AppData/Local/Temp/claude/C--Users-xavie-Desktop-Exo-OS/a9d46684-607e-4afa-a39d-9299bf8585b1/tasks/b2fhbu3o3.output"
n=0
while [ "$n" -lt 175 ]; do
  if grep -qaE "log size|BUILDFAIL|error\[" "$F" 2>/dev/null; then break; fi
  sleep 3
  n=$((n + 1))
done
L=/mnt/c/Users/xavie/Desktop/Exo-OS/tools/e9d25b.txt
C() { tr -cd '\11\12\15\40-\176' < "$L"; }
echo "=== SIGF (signal livré : restorer=0 => saut NULL au retour du handler) ==="
C | grep -aoE '<SIGF s=[0-9a-f]+ h=[0-9a-f]+ restorer=[0-9a-f]+ ursp=[0-9a-f]+>' | head -8
echo "=== contexte SIGF vs SEGV ==="
C | grep -aoE '<SIGF[^>]*>|init: spawnret [a-z_]+ pid=[0-9]+|<PF p1 a=0+ X>|<SEGV>' | tail -10
echo "=== build tail ==="
grep -aE "log size|BUILDFAIL|error\[" "$F" | tail -3
