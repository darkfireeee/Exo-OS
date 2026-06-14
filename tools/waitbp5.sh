#!/bin/bash
F="/mnt/c/Users/xavie/AppData/Local/Temp/claude/C--Users-xavie-Desktop-Exo-OS/a9d46684-607e-4afa-a39d-9299bf8585b1/tasks/bp5ub9quo.output"
n=0
while [ "$n" -lt 175 ]; do
  if grep -qaE "log size|BUILDFAIL|error\[" "$F" 2>/dev/null; then break; fi
  sleep 3
  n=$((n + 1))
done
L=/mnt/c/Users/xavie/Desktop/Exo-OS/tools/e9d25b.txt
C() { tr -cd '\11\12\15\40-\176' < "$L"; }
echo "=== PTE readback (M=mappée→TLB/CR3 ; UNMAP=démappée) ==="
C | grep -aoE '<PTE [^>]*>' | sort | uniq -c | head -15
echo "=== CONSOLE progression ==="
C | grep -aoE 'init[_a-z]*: [a-zA-Z_/, =0-9-]+|[a-z_]+_server: [a-zA-Z ]+|exosh[^<]+|init: chk [a-z_]+' | head -30
echo "=== EXEC + K0/LKP (readiness ?) ==="
C | grep -aoE '<EXEC [^>]*>|<K0 [^>]*>|<LKP [^>]*>' | sort | uniq -c | head
echo "=== build tail ==="
grep -aE "log size|BUILDFAIL|error\[" "$F" | tail -3
