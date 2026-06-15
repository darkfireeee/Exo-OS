#!/bin/bash
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
L=tools/e9d25b.txt
C() { tr -cd '\11\12\15\40-\176' < "$L"; }
echo "=== console init (ordre) ==="
C | grep -aoE 'init[_a-z]*: [a-zA-Z_/, =0-9-]+' | tail -25
echo "=== marqueurs détecteurs ==="
echo "SIGF=$(C | grep -aoc '<SIGF') FREEAS=$(C | grep -aoc '<FREEAS') SEGV=$(C | grep -aoc '<SEGV') STK=$(C | grep -aoc '<STK ')"
echo "PFp1=$(C | grep -aoEc '<PF p1 a=') nullX=$(C | grep -aoEc '<PF p1 a=0+ X>')"
echo "=== SIGF détails ==="
C | grep -aoE '<SIGF s=[0-9a-f]+ h=[0-9a-f]+ restorer=[0-9a-f]+ ursp=[0-9a-f]+>' | head -8
echo "=== STK détails ==="
C | grep -aoE '<STK [^>]*>' | head -3
echo "=== dernières fautes p1 ==="
C | grep -aoE '<PF p1 a=[0-9a-f]+ [XRW]>' | tail -5
