#!/bin/bash
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
L=tools/e9d25b.txt
C() { tr -cd '\11\12\15\40-\176' < "$L"; }
echo "=== séquence pr/CGe/CGw autour du hang ==="
C | grep -aoE 'init: pr[0-9]|<CGe>|<CGw>|<CGwE>' | tail -16
echo "=== compteurs ==="
echo "CGe=$(C | grep -aoc '<CGe>') CGw=$(C | grep -aoc '<CGw>') CGwE=$(C | grep -aoc '<CGwE>') pr3=$(C | grep -aoc 'init: pr3') pr4=$(C | grep -aoc 'init: pr4')"
