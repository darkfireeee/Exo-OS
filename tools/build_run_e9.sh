#!/bin/bash
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
pkill -9 -f qemu-system 2>/dev/null
echo "=== make iso ==="
make iso 2>&1 | grep -iE 'error\[|error:|undefined|cannot find|OK.*ISO' | tail -6
LOG=/tmp/e9p.txt; ERR=/tmp/qerr.txt; INT=/tmp/qint.log
rm -f "$LOG" "$ERR" "$INT"
echo "=== run QEMU (flags canoniques) ==="
timeout 28 qemu-system-x86_64 \
  -machine q35 -m 256M -boot d -vga std -serial null \
  -no-reboot -no-shutdown \
  -d int,cpu_reset -D "$INT" \
  -debugcon "file:$LOG" \
  -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
  -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  -cdrom exo-os.iso -display none 2>"$ERR"
echo "=== FAULT dumps (#PF/#GP/#DF) ==="
grep -aoE '#PF cr2=[0-9a-f]+ rip=[0-9a-f]+|#GP cr2=[0-9a-f]+ rip=[0-9a-f]+|\[#DF\][^\\]*' "$LOG" 2>/dev/null | tail -10
echo "=== tail markers ==="
grep -aoE 'V[0-9abcdX]|#[0-9]' "$LOG" 2>/dev/null | tr '\n' ' ' | tail -c 60
echo ""
echo "=== resets(INT08) ==="; grep -ac 'Servicing hardware INT=0x08' "$INT" 2>/dev/null
echo "=== log size ==="; wc -c "$LOG" 2>/dev/null
