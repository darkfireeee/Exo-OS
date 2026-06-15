#!/bin/bash
F="/mnt/c/Users/xavie/AppData/Local/Temp/claude/C--Users-xavie-Desktop-Exo-OS/a9d46684-607e-4afa-a39d-9299bf8585b1/tasks/bhrf040qd.output"
n=0
while [ "$n" -lt 175 ]; do
  if grep -qaE "log size|BUILDFAIL|error\[" "$F" 2>/dev/null; then break; fi
  sleep 3
  n=$((n + 1))
done
L=/mnt/c/Users/xavie/Desktop/Exo-OS/tools/e9d25b.txt
C() { tr -cd '\11\12\15\40-\176' < "$L"; }
echo "=== FRMBAD (rcx/rsp before/after dispatch — corruption pendant le syscall ?) ==="
C | grep -aoE '<FRMBAD nr=[0-9a-f]+ rcxB=[0-9a-f]+ rcxA=[0-9a-f]+ rspB=[0-9a-f]+ rspA=[0-9a-f]+>' | head -8
echo "=== ordre FRMBAD vs monoC/CGwOK/SEGV ==="
C | grep -aoE '<FRMBAD[^>]*>|init: monoC|<CGwOK>|init: spawnret [a-z_]+ pid=[0-9]+|<SEGV>' | tail -10
echo "=== build tail ==="
grep -aE "log size|BUILDFAIL|error\[" "$F" | tail -3
