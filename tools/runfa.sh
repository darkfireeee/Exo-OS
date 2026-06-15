#!/bin/bash
# Probe FA : toutes les fautes Ring3 d'init. Adresse répétée = boucle #PF ;
# adresses variées = progression (lente). Init est propre (aucune instrumentation).
source ~/.profile 2>/dev/null; export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
pkill -9 -f qemu-system 2>/dev/null; sleep 1
touch kernel/src/arch/x86_64/exceptions.rs servers/init_server/src/*.rs
make iso >/tmp/mk.log 2>&1 && echo "ISOOK" || { echo "BUILDFAIL"; grep -iE "error\[|error:" /tmp/mk.log | head -20; exit 1; }
cargo run -q -p exofs-mkroot -- --image target/qemu/exofs-root.img --size 536870912 --root target/exofs-rootfs 2>&1 | tail -1
LOG=tools/e9fa.txt; rm -f "$LOG"
timeout 90 qemu-system-x86_64 \
  -machine q35 -m 256M -boot d -vga std -serial null -no-reboot -no-shutdown \
  -debugcon "file:$LOG" \
  -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  -cdrom exo-os.iso -display none 2>/dev/null
echo "=== derniers logs init/ipc ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE 'init: [a-zA-Z_/, =0-9-]+|ipc_router: [a-z]+' | tail -6
echo "=== FA : toutes les fautes init (addr cause present) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<FA [0-9a-f]+ [XWR] p=[01]>'
echo "=== FA : adresses uniques (répétition = boucle) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<FA [0-9a-f]+ [XWR]' | sort | uniq -c | sort -rn | head
echo "=== total FA ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoc '<FA '
echo "=== log size ==="; wc -c "$LOG"
