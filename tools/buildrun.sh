#!/bin/bash
source ~/.profile 2>/dev/null; export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
pkill -9 -f qemu-system 2>/dev/null; sleep 1
# Touch uniquement les fichiers edites (build incremental plus rapide).
touch kernel/src/memory/physical/allocator/buddy.rs kernel/src/fs/exofs/cache/blob_cache.rs
make iso >/tmp/mk.log 2>&1 && echo "ISOOK" || { echo "BUILDFAIL"; grep -iE "error\[|error:" /tmp/mk.log | head -12; exit 1; }
cargo run -q -p exofs-mkroot -- --image target/qemu/exofs-root.img --size 536870912 --root target/exofs-rootfs 2>&1 | tail -1
LOG=/tmp/e9br.txt; rm -f "$LOG"
timeout 30 qemu-system-x86_64 \
  -machine q35 -m 256M -boot d -vga std -serial null -no-reboot -no-shutdown \
  -debugcon "file:$LOG" \
  -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  -cdrom exo-os.iso -display none 2>/dev/null
STREAM=/tmp/evstream.txt
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<BPG [^>]*>|<INS [^>]*>|<MAP#[0-9a-f]{16}[^>]*>|[a-z_]+: [a-z]+' > "$STREAM"
echo "=== COMPTES ==="
echo "BPG A fl=0000 (SLUB) : $(grep -ac 'BPG A.*fl=0000' "$STREAM")"
echo "BPG A fl=010c (virtio DMA): $(grep -ac 'BPG A.*fl=010c' "$STREAM")"
echo "BPG A autres fl       : $(grep -aE 'BPG A' "$STREAM" | grep -avcE 'fl=0000|fl=010c')"
echo "BPG F (free)          : $(grep -ac 'BPG F' "$STREAM")"
echo "INS                   : $(grep -ac '<INS' "$STREAM")"
echo "=== CONTEXTE autour des <INS> (15 evts avant/apres) ==="
grep -anE '<INS|<MAP#' "$STREAM" | head -4
echo "--- fenetre ---"
awk '/<INS/{print NR": "$0; for(i=1;i<=0;i++){}}' "$STREAM" | head -4
nl -ba "$STREAM" | grep -E '<INS|<MAP#' | head -6
echo "=== Les allocs SLUB (fl=0000) de 0xff33000 et ce qui suit (8 evts) ==="
grep -anE 'BPG A.*fl=0000' "$STREAM" | head -8
