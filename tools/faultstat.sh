#!/bin/bash
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
L=tools/e9d25b.txt
C() { tr -cd '\11\12\15\40-\176' < "$L"; }
echo "faults p1 distinctes: $(C | grep -aoE '<PF p1 a=[0-9a-f]+' | sort -u | wc -l)"
echo "faults p1 total     : $(C | grep -aoEc '<PF p1 a=')"
echo "--- top adresses p1 (répétées = bloqué) ---"
C | grep -aoE '<PF p1 a=[0-9a-f]+' | sort | uniq -c | sort -rn | head -8
echo "--- dernières adresses p1 (où ça en est) ---"
C | grep -aoE '<PF p1 a=[0-9a-f]+' | tail -8
