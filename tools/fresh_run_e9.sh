#!/bin/bash
source ~/.bashrc 2>/dev/null
source ~/.profile 2>/dev/null
export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
pkill -9 -f qemu-system 2>/dev/null
sleep 1
echo "=== rebuild iso ==="
make iso >/tmp/mk.log 2>&1 && echo "iso ok" || { echo "BUILD FAIL"; grep -iE 'error' /tmp/mk.log | tail -5; exit 1; }
echo "=== regen disque frais ==="
cargo run -q -p exofs-mkroot -- --image target/qemu/exofs-root.img --size 536870912 --root target/exofs-rootfs 2>&1 | tail -1
LOG=/tmp/e9p.txt; ERR=/tmp/qerr.txt; INT=/tmp/qint.log
rm -f "$LOG" "$ERR" "$INT"
echo "=== run QEMU (disque frais, flags canoniques) ==="
timeout 28 qemu-system-x86_64 \
  -machine q35 -m 256M -boot d -vga std -serial null \
  -no-reboot -no-shutdown \
  -d int,cpu_reset -D "$INT" \
  -debugcon "file:$LOG" \
  -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
  -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  -cdrom exo-os.iso -display none 2>"$ERR"
echo "=== sequence marqueurs (avant 1er fault) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '@[0-9a-e]|#[0-9]|V[0-9abcdX]|[RTE@]boot|kdb|PROCESS|SECURITY|STAGE0|IPC|stage ok FS|#PF|#GP|PGD' | tr '\n' ' ' | head -c 400
echo ""
echo "=== 1er #PF + contexte ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE 'R|T|E|@|#PF cr2=[0-9a-f]+ rip=[0-9a-f]+' | head -6
echo "=== FAULT dumps (5 premiers) ==="
grep -aoE '#PF cr2=[0-9a-f]+ rip=[0-9a-f]+|#GP cr2=[0-9a-f]+ rip=[0-9a-f]+|\[#DF\][^\\]{0,60}' "$LOG" 2>/dev/null | head -5
echo ""
echo "=== resets(INT08) ==="; grep -ac 'Servicing hardware INT=0x08' "$INT" 2>/dev/null
echo "=== sizes ==="; wc -c "$LOG"
