#!/bin/bash
source ~/.profile 2>/dev/null; export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
F="/mnt/c/Users/xavie/AppData/Local/Temp/claude/C--Users-xavie-Desktop-Exo-OS/a9d46684-607e-4afa-a39d-9299bf8585b1/tasks/bntffinh6.output"
i=0
while [ $i -lt 120 ]; do
  if grep -qE "ISOOK|BUILDFAIL" "$F" 2>/dev/null; then break; fi
  sleep 3
  i=$((i+1))
done
echo "build:"; cat "$F" 2>/dev/null
if ! grep -qE "ISOOK" "$F" 2>/dev/null; then echo "NOT BUILT, abort"; exit 1; fi
pkill -9 -f qemu-system 2>/dev/null; sleep 1
cargo run -q -p exofs-mkroot -- --image target/qemu/exofs-root.img --size 536870912 --root target/exofs-rootfs 2>&1 | tail -1
LOG=/tmp/e9map.txt; rm -f "$LOG"
timeout 30 qemu-system-x86_64 \
  -machine q35 -m 256M -boot d -vga std -serial null -no-reboot -no-shutdown \
  -debugcon "file:$LOG" \
  -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  -cdrom exo-os.iso -display none 2>/dev/null
echo "=== MAP dumps ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<MAP#[^>]*>'
echo "=== progression ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '[a-z_]+: [a-z]+|<INS [^>]*>' | tail -8
