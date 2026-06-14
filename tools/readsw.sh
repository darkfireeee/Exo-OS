#!/bin/bash
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
L=tools/e9d25b.txt
clean() { tr -cd '\11\12\15\40-\176' < "$L"; }
echo "=== SW transitions (toutes, head 30) ==="
clean | grep -aoE '<SW [^>]*>' | head -30
echo
echo "=== SW impliquant pid 1 (init) ==="
clean | grep -aoE '<SW [^>]*>' | grep -a 00000001 | head -15
echo
echo "=== CR3 faults uniq (cr=hw) ==="
clean | grep -aoE '<CR3 [^>]*>' | sort | uniq -c | head -8
