#!/bin/bash
F="/mnt/c/Users/xavie/AppData/Local/Temp/claude/C--Users-xavie-Desktop-Exo-OS/a9d46684-607e-4afa-a39d-9299bf8585b1/tasks/bx2gr46iw.output"
n=0
while [ "$n" -lt 175 ]; do
  if grep -qaE "log size|BUILDFAIL|error\[" "$F" 2>/dev/null; then break; fi
  sleep 3
  n=$((n + 1))
done
if grep -qaE "BUILDFAIL|error\[" "$F" 2>/dev/null; then
  echo "BUILD FAIL"; grep -aE "error\[|error:" "$F" | head -10; exit 1
fi
L=/mnt/c/Users/xavie/Desktop/Exo-OS/tools/e9d25b.txt
C() { tr -cd '\11\12\15\40-\176' < "$L"; }
echo "=== FRMBAD (rcx/rsp corrompus pendant dispatch ?) ==="
C | grep -aoE '<FRMBAD nr=[0-9a-f]+ rcx [0-9a-f]+->[0-9a-f]+ rsp [0-9a-f]+->[0-9a-f]+>' | head -8
echo "total FRMBAD=$(C | grep -aoc '<FRMBAD')"
echo "=== séquence pr/CGe/CGw/FRMBAD ==="
C | grep -aoE 'init: pr[0-9]|<CGe>|<CGw>|<FRMBAD[^>]*>' | tail -12
echo "log: $(wc -c < "$L")"
