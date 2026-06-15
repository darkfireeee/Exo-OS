#!/bin/bash
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
L=tools/e9d25b.txt
C() { tr -cd '\11\12\15\40-\176' < "$L"; }
echo "=== FRMBAD (corruption frame.rcx/rsp pendant dispatch) ==="
C | grep -aoE '<FRMBAD nr=[0-9a-f]+ rcxB=[0-9a-f]+ rcxA=[0-9a-f]+ rspB=[0-9a-f]+ rspA=[0-9a-f]+>' | head -8
echo "total FRMBAD=$(C | grep -aoc '<FRMBAD')"
echo "=== ordre final ==="
C | grep -aoE '<FRMBAD[^>]*>|init: monoC|<CGe>|<CGwOK>|init: spawnret [a-z_]+ pid=[0-9]+|init: pr[0-9]-[a-z-]+|<SEGV>' | tail -12
