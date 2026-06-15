#!/bin/bash
F="/mnt/c/Users/xavie/AppData/Local/Temp/claude/C--Users-xavie-Desktop-Exo-OS/a9d46684-607e-4afa-a39d-9299bf8585b1/tasks/by9vpsgiz.output"
n=0
while [ "$n" -lt 80 ]; do
  if grep -qaE "log size" "$F" 2>/dev/null; then break; fi
  sleep 3
  n=$((n + 1))
done
L=/mnt/c/Users/xavie/Desktop/Exo-OS/tools/e9d180.txt
C() { tr -cd '\11\12\15\40-\176' < "$L"; }
echo "=== console init/serveurs/shell (progression complète) ==="
C | grep -aoE 'init[_a-z]*: [a-zA-Z_/, =0-9-]+|[a-z_]+_server: [a-zA-Z ]+|exosh[^<]*|ExoSH[^<]*|\$ |exo-shield[^<]*' | tail -45
echo "=== EXEC serveurs ==="
C | grep -aoE '<EXEC [^>]*>' | sort | uniq -c
echo "=== shell atteint ? ==="
C | grep -aoiE 'exosh|welcome|\$ |# |prompt' | tail -5
echo "log: $(wc -c < "$L")"
