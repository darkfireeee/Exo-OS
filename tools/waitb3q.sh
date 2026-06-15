#!/bin/bash
F="/mnt/c/Users/xavie/AppData/Local/Temp/claude/C--Users-xavie-Desktop-Exo-OS/a9d46684-607e-4afa-a39d-9299bf8585b1/tasks/b3qhwa85o.output"
n=0
while [ "$n" -lt 175 ]; do
  if grep -qaE "log size|BUILDFAIL|error\[" "$F" 2>/dev/null; then break; fi
  sleep 3
  n=$((n + 1))
done
L=/mnt/c/Users/xavie/Desktop/Exo-OS/tools/e9d25b.txt
C() { tr -cd '\11\12\15\40-\176' < "$L"; }
echo "=== CWB @ad0 (contenu source AVANT copie a l offset corruption) ==="
C | grep -aoE '<CWB f=[0-9a-f]+ rc=[0-9a-f]+ (RIP|COPY) @ad0=[0-9a-f]+,[0-9a-f]+>' | head -6
echo "=== STK (contenu @crash : compare avec CWB COPY @ad0) ==="
C | grep -aoE '<STK rsp=00007ffffffefad0[^>]+' | head -1
echo "=== build tail ==="
grep -aE "log size|BUILDFAIL|error\[" "$F" | tail -3
