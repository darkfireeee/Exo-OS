#!/bin/bash
# Run-only (iso deja a jour). Capture le flux d'evenements dans un fichier
# PERSISTANT (repo) et montre la fenetre autour de l'alloc SLUB + insert2.
source ~/.profile 2>/dev/null; export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
pkill -9 -f qemu-system 2>/dev/null; sleep 1
cargo run -q -p exofs-mkroot -- --image target/qemu/exofs-root.img --size 536870912 --root target/exofs-rootfs 2>&1 | tail -1
LOG=tools/e9run.txt; rm -f "$LOG"
timeout 30 qemu-system-x86_64 \
  -machine q35 -m 256M -boot d -vga std -serial null -no-reboot -no-shutdown \
  -debugcon "file:$LOG" \
  -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  -cdrom exo-os.iso -display none 2>/dev/null
STREAM=tools/evfull.txt
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<BPG [^>]*>|<INS [^>]*>|<MAP#0+[0-9a-f] [^>]*>' > "$STREAM"
echo "total evts: $(wc -l < $STREAM)"
echo "=== position alloc SLUB (fl=0000) ==="; grep -an 'fl=0000' "$STREAM"
echo "=== positions INS ==="; grep -an '<INS' "$STREAM"
SL=$(grep -an 'fl=0000' "$STREAM" | head -1 | cut -d: -f1)
I2=$(grep -an '<INS' "$STREAM" | sed -n 2p | cut -d: -f1)
echo "=== fenetre SLUB-alloc($SL) .. insert2($I2) [ce qui ecrit le noeud] ==="
awk -v a="$SL" -v b="$I2" 'NR>=a-2 && NR<=b+2' "$STREAM" | nl -ba | sed -E 's/ nw0=.*//; s/ L8=.*//'
