#!/bin/bash
source ~/.profile 2>/dev/null; export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
pkill -9 -f qemu-system 2>/dev/null; sleep 1
cargo run -q -p exofs-mkroot -- --image target/qemu/exofs-root.img --size 536870912 --root target/exofs-rootfs 2>&1 | tail -1
LOG=tools/e9d180.txt; rm -f "$LOG"
timeout 180 qemu-system-x86_64 \
  -machine q35 -m 256M -boot d -vga std -serial null -no-reboot -no-shutdown \
  -debugcon "file:$LOG" \
  -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  -cdrom exo-os.iso -display none 2>/dev/null
C() { tr -cd '\11\12\15\40-\176' < "$LOG"; }
echo "=== console init (fin) ==="
C | grep -aoE 'init[_a-z]*: [a-zA-Z_/, =0-9-]+' | tail -12
echo "=== SIGF (restorer=0 ?) ==="
C | grep -aoE '<SIGF s=[0-9a-f]+ h=[0-9a-f]+ restorer=[0-9a-f]+ ursp=[0-9a-f]+>' | head -8
echo "=== compteurs ==="
echo "SIGF=$(C | grep -aoc '<SIGF') FREEAS=$(C | grep -aoc '<FREEAS') SEGV=$(C | grep -aoc '<SEGV') nullX=$(C | grep -aoEc '<PF p1 a=0+ X>')"
echo "=== STK au saut NULL ==="
C | grep -aoE '<STK [^>]*>' | head -3
echo "=== log size ==="; wc -c "$LOG"
