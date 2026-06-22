#!/bin/bash
# fast_iso (strip + diag #25 cow.rs) + boot Bochs (yes c, sans breakpoint = rapide)
# pour lire le frame physique (F') de la pile user d'init via les marqueurs <F25>.
source ~/.profile 2>/dev/null; export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
bash tools/fast_iso.sh || { echo "FAST_ISO FAIL"; exit 1; }
pkill -9 bochs Xvfb 2>/dev/null; sleep 1
Xvfb :99 -screen 0 640x480x8 >/tmp/xvfb.log 2>&1 & sleep 2
export DISPLAY=:99
rm -f /tmp/bochsfind.txt
yes c | timeout 150 bochs -q -f tools/bochsrc >/tmp/bochsfind.txt 2>&1
echo "=== <F25 p= f=> markers (init stack page -> frame phys) ==="
tr -cd "\11\12\15\40-\176\n" < /tmp/bochsfind.txt 2>/dev/null | grep -aoE "<F25 p=[0-9a-f]+ f=[0-9a-f]+>" | sort -u | head -12
echo "=== SEGV (#25 reproduced?) ==="
tr -cd "\11\12\15\40-\176\n" < /tmp/bochsfind.txt 2>/dev/null | grep -aoE "SEGV pid=1" | head -1
echo "DONE"
