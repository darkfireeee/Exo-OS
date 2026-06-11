#!/bin/bash
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
pkill -9 -f qemu-system 2>/dev/null
sleep 1
LOG=/tmp/e9p.txt
rm -f "$LOG"
timeout 25 qemu-system-x86_64 \
  -machine q35 -m 256M -boot d -vga std -serial null -no-reboot -no-shutdown \
  -debugcon "file:$LOG" \
  -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
  -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  -cdrom exo-os.iso -display none 2>/dev/null
echo "=== sequence complete (marqueurs + faults) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | sed 's/boot_display: stage ok /@OK:/g' | grep -aoE '@OK:[A-Z0-9]+|@[0-9a-e]|#[0-9]|V[0-9abcdX]|[RTE]@?|#PF cr2=[0-9a-f]+ rip=[0-9a-f]+' | tr '\n' ' ' | head -c 600
echo ""
echo "=== compte #PF ==="; grep -ac '#PF cr2=' "$LOG"
echo "=== raw tail (300 derniers chars) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | tail -c 300
echo ""
echo "=== size ==="; wc -c "$LOG"
