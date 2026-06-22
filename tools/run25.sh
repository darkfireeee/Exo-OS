#!/bin/bash
# DIAG #25 : build avec la sonde buddy + boot QEMU normal (sans gdb, pour que la
# course se déclenche) + extraction des marqueurs <25A>/<25F> (alloc/free de F').
source ~/.profile 2>/dev/null; export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
pkill -9 qemu-system-x86 2>/dev/null; sleep 1
echo "=== make iso ==="
make iso >/tmp/mk25.log 2>&1 && echo "ISOOK" || { echo "BUILDFAIL"; grep -iE "error\[|error:|cannot find" /tmp/mk25.log | head -20; exit 1; }
LOG=/tmp/e9_25.txt; rm -f "$LOG"
echo "=== run QEMU 30s (normal, race active) ==="
timeout 30 qemu-system-x86_64 \
  -machine q35 -m 256M -boot d -vga std -serial null -no-reboot -no-shutdown \
  -debugcon "file:$LOG" \
  -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
  -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  -cdrom exo-os.iso -display none 2>/dev/null
echo "=== <25DBL>/<25TAINT> markers (double-alloc OU réutilisation d'une frame DMA) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<25(DBL|TAINT)[^>]*>' | head -30
echo "=== SEGV ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<SEGV pid=1[^>]*' | head -2
echo "=== boot progress (services / ready / shell / panic) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE 'init: (start|spawned|ready) [a-z_]+|reached shell|shell|login|PANIC|RESURRECTION|panic' | tail -25
echo "=== log size ==="; wc -c "$LOG"
