#!/bin/bash
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
L=tools/e9d25b.txt
# Séquence chronologique des marqueurs clés (DPM map, FREEAS, PTE re-fault, EXEC)
tr -cd '\11\12\15\40-\176' < "$L" \
  | grep -aoE '<DPM [^>]*>|<FREEAS root=[0-9a-f]+>|<PTE [^>]*>|<EXEC [^>]*>|<CRT [^>]*>' \
  | awk '
    /DPM/   {dpm++; if(dpm<=2||dpm%10==0) print "  [DPM #"dpm"] "$0; next}
    /PTE M/ {ptem++; print "  [PTE-M] "$0; next}
    /PTE UNMAP/ {unmap++; if(unmap<=3||unmap%5==0) print "  [UNMAP #"unmap"] "$0; next}
    {print ">>> "$0}
  '
echo "--- totaux : DPM=$(tr -cd '\11\12\15\40-\176' < "$L" | grep -aoc "<DPM ") FREEAS=$(tr -cd '\11\12\15\40-\176' < "$L" | grep -aoc "<FREEAS") UNMAP=$(tr -cd '\11\12\15\40-\176' < "$L" | grep -aoc "<PTE UNMAP")"
