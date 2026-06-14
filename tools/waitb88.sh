#!/bin/bash
F="/mnt/c/Users/xavie/AppData/Local/Temp/claude/C--Users-xavie-Desktop-Exo-OS/a9d46684-607e-4afa-a39d-9299bf8585b1/tasks/b88s90v9e.output"
n=0
while [ "$n" -lt 175 ]; do
  if grep -qaE "log size|BUILDFAIL|error\[" "$F" 2>/dev/null; then break; fi
  sleep 3
  n=$((n + 1))
done
L=/mnt/c/Users/xavie/Desktop/Exo-OS/tools/e9d25b.txt
C() { tr -cd '\11\12\15\40-\176' < "$L"; }
echo "=== DPM (PTE juste après map : P1=ok P0=map cassé ; f=frame) ==="
C | grep -aoE '<DPM [^>]*>' | sort | uniq -c | head -20
echo "=== FREEAS (roots PML4 libérés — init=0ff43000 ?) ==="
C | grep -aoE '<FREEAS root=[0-9a-f]+>' | sort | uniq -c | head
echo "=== PTE readback re-fault (M/UNMAP) ==="
C | grep -aoE '<PTE [^>]*>' | sort | uniq -c | head -8
echo "=== CR3 (init=0ff43000) ==="
C | grep -aoE '<CR3 as=[0-9a-f]+ [^>]*cr=[0-9a-f]+' | sort | uniq -c | head -5
echo "=== EXEC/K0/LKP ==="
C | grep -aoE '<EXEC [^>]*>|<K0 [^>]*>|<LKP [^>]*>' | sort | uniq -c | head
echo "=== build tail ==="
grep -aE "log size|BUILDFAIL|error\[" "$F" | tail -3
