#!/bin/bash
source ~/.profile 2>/dev/null; export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
pkill -9 -f qemu-system 2>/dev/null; sleep 1
touch kernel/src/fs/elf_loader_impl.rs kernel/src/memory/physical/allocator/buddy.rs kernel/src/fs/exofs/cache/blob_cache.rs
make iso >/tmp/mk.log 2>&1 && echo "ISOOK" || { echo "BUILDFAIL"; grep -iE "error\[|error:" /tmp/mk.log | head -12; exit 1; }
cargo run -q -p exofs-mkroot -- --image target/qemu/exofs-root.img --size 536870912 --root target/exofs-rootfs 2>&1 | tail -1
LOG=tools/e9fix.txt; rm -f "$LOG"
timeout 40 qemu-system-x86_64 \
  -machine q35 -m 256M -boot d -vga std -serial null -no-reboot -no-shutdown \
  -debugcon "file:$LOG" \
  -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  -cdrom exo-os.iso -display none 2>/dev/null
echo "=== PROGRESSION serveurs/boot (markers X: Y) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '[a-z_]+: [a-zA-Z_]+' | uniq -c | tail -40
echo "=== spin BlobCache encore present ? (MAP dumps / INS) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aocE '<MAP#|<INS '
echo "=== shell / exosh / prompt ? ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoiE 'exosh|shell|prompt|\$ |welcome|login' | head
echo "=== taille log ==="; wc -c "$LOG"
