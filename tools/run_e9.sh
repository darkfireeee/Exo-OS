#!/bin/bash
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
pkill -9 -f qemu-system >/dev/null 2>&1 || true
sleep 1
LOG=/tmp/e9p.txt
ERR=/tmp/qerr.txt
rm -f "$LOG" "$ERR"
timeout 30 qemu-system-x86_64 \
  -machine q35 -m 256M -boot d -vga std -serial null \
  -no-reboot -no-shutdown \
  -debugcon "file:$LOG" \
  -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
  -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  -cdrom exo-os.iso -display none 2>"$ERR" || true
echo "qemu exit: $?"
echo "=== qemu stderr ==="
head -6 "$ERR" 2>/dev/null || true
echo "=== log size ==="
wc -c "$LOG" 2>/dev/null || true
echo "=== last log lines ==="
tail -n 25 "$LOG" 2>/dev/null || true
