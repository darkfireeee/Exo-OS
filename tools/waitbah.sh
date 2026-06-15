#!/bin/bash
F="/mnt/c/Users/xavie/AppData/Local/Temp/claude/C--Users-xavie-Desktop-Exo-OS/a9d46684-607e-4afa-a39d-9299bf8585b1/tasks/bahv7vry2.output"
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
echo "=== séquence finale (spawnret/pr/CGe/CGw — où ça hang) ==="
C | grep -aoE 'init: spawned [a-z_]+ pid=[0-9]+|init: pr[0-9]|<CGe>|<CGw>|<CGwE>|init: wave-done, poll|init: rdy [a-z_]+ [a-zA-Z,+]+|init: start [a-z_]+' | tail -25
echo "=== compteurs ==="
echo "pr1=$(C | grep -aoc 'init: pr1') pr2=$(C | grep -aoc 'init: pr2') pr3=$(C | grep -aoc 'init: pr3') pr4=$(C | grep -aoc 'init: pr4') CGe=$(C | grep -aoc '<CGe>') CGw=$(C | grep -aoc '<CGw>')"
echo "log size: $(wc -c < "$L")"
