#!/bin/bash
# Probe F1 : détecter boucle #PF d'init (pid1) + mismatch table CoW (as) vs CR3 (tk).
source ~/.profile 2>/dev/null; export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
pkill -9 -f qemu-system 2>/dev/null; sleep 1
touch kernel/src/arch/x86_64/exceptions.rs
make iso >/tmp/mk.log 2>&1 && echo "ISOOK" || { echo "BUILDFAIL"; grep -iE "error\[|error:" /tmp/mk.log | head -20; exit 1; }
cargo run -q -p exofs-mkroot -- --image target/qemu/exofs-root.img --size 536870912 --root target/exofs-rootfs 2>&1 | tail -1
LOG=tools/e9f1.txt; rm -f "$LOG"
timeout 60 qemu-system-x86_64 \
  -machine q35 -m 256M -boot d -vga std -serial null -no-reboot -no-shutdown \
  -debugcon "file:$LOG" \
  -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  -cdrom exo-os.iso -display none 2>/dev/null
echo "=== fin de boot (logs init/ipc) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE 'init: [a-zA-Z_/, =0-9-]+|ipc_router: [a-z]+' | tail -8
echo "=== F1 (faute write init: a=addr tk=cr3thread as=pml4_AS) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<F1 [^>]*>'
echo "=== F1 count + adresses uniques ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<F1 a=[0-9a-f]+' | sort | uniq -c
echo "=== log size ==="; wc -c "$LOG"
