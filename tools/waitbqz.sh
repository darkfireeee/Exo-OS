#!/bin/bash
F="/mnt/c/Users/xavie/AppData/Local/Temp/claude/C--Users-xavie-Desktop-Exo-OS/a9d46684-607e-4afa-a39d-9299bf8585b1/tasks/bqzfoh49c.output"
n=0
while [ "$n" -lt 175 ]; do
  if grep -qaE "log size|BUILDFAIL|error\[" "$F" 2>/dev/null; then break; fi
  sleep 3
  n=$((n + 1))
done
L=/mnt/c/Users/xavie/Desktop/Exo-OS/tools/e9d25b.txt
C() { tr -cd '\11\12\15\40-\176' < "$L"; }
echo "=== CWB avec dump source (w0,w1 = contenu AVANT copie/break) ==="
C | grep -aoE '<CWB f=[0-9a-f]+ rc=[0-9a-f]+ (RIP|COPY) @[0-9a-f]+,[0-9a-f]+>' | head -10
echo "=== ordre CWB vs spawnret/SEGV + STK ==="
C | grep -aoE '<CWB[^>]*>|init: spawnret [a-z_]+ pid=[0-9]+|<FREEAS root=[0-9a-f]+>|<SEGV>' | tail -10
C | grep -aoE '<STK rsp=[0-9a-f]+ rbp=[0-9a-f]+ spf=[0-9a-fUNMAPED-]+ \| [0-9a-f ]+' | head -1
echo "=== build tail ==="
grep -aE "log size|BUILDFAIL|error\[" "$F" | tail -3
