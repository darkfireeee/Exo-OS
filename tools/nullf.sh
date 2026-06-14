#!/bin/bash
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
L=tools/e9d25b.txt
C() { tr -cd '\11\12\15\40-\176' < "$L"; }
echo "=== faute @0 : type R/W/X (PF marker complet) ==="
C | grep -aoE '<PF p[0-9]+ a=0+ [RWX]>|<PF p[0-9]+ a=0000000000000000[^<]*' | head
echo "=== contexte autour de a=00..00 (PF bruts) ==="
C | grep -aoE '<PF [^<]*0000000000000000[^<]*' | head
echo "=== SEGV / SIGSEGV / kill / die ==="
C | grep -aoE '<SEGV[^<]*|<POOM[^<]*|SIGSEGV|init.*(die|exit|crash|fault)' | head
echo "=== derniers 400 caractères du log (fin de vie d init) ==="
C | tail -c 400
