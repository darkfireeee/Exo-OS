#!/bin/bash
F="/mnt/c/Users/xavie/AppData/Local/Temp/claude/C--Users-xavie-Desktop-Exo-OS/a9d46684-607e-4afa-a39d-9299bf8585b1/tasks/botgz112n.output"
n=0
while [ "$n" -lt 175 ]; do
  if grep -qaE "log size|BUILDFAIL|error\[" "$F" 2>/dev/null; then break; fi
  sleep 3
  n=$((n + 1))
done
L=/mnt/c/Users/xavie/Desktop/Exo-OS/tools/e9d25b.txt
C() { tr -cd '\11\12\15\40-\176' < "$L"; }
echo "=== CWB (CoW break pile : frame / refcount / RIP|COPY) ==="
C | grep -aoE '<CWB f=[0-9a-f]+ rc=[0-9a-f]+ (RIP|COPY)>' | head -24
echo "total CWB=$(C | grep -aoc '<CWB')"
echo "=== ordre CWB vs spawnret/FREEAS/SEGV/STK ==="
C | grep -aoE '<CWB[^>]*>|init: spawnret [a-z_]+ pid=[0-9]+|<FREEAS root=[0-9a-f]+>|<SEGV>' | tail -18
echo "=== STK (frame pile spf) ==="
C | grep -aoE '<STK rsp=[0-9a-f]+ rbp=[0-9a-f]+ spf=[0-9a-fUNMAPED-]+' | head -2
echo "=== build tail ==="
grep -aE "log size|BUILDFAIL|error\[" "$F" | tail -3
