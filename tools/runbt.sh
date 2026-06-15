#!/bin/bash
# Boot-test (sans rebuild) : l'enforcement capability TIER 0 casse-t-il le boot ?
source ~/.profile 2>/dev/null; export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
pkill -9 -f qemu-system 2>/dev/null; sleep 1
cargo run -q -p exofs-mkroot -- --image target/qemu/exofs-root.img --size 536870912 --root target/exofs-rootfs 2>&1 | tail -1
LOG=tools/e9bt.txt; rm -f "$LOG"
timeout 60 qemu-system-x86_64 \
  -machine q35 -m 256M -boot d -vga std -serial null -no-reboot -no-shutdown \
  -debugcon "file:$LOG" \
  -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  -cdrom exo-os.iso -display none 2>/dev/null
echo "=== progression boot ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE 'init: [a-zA-Z_/, =0-9-]+|[a-z0-9_]+: (boot|registered|ready|started)|<EXEC [^>]*>|exosh' | tail -25
echo "=== anomalies (EACCES/perm/SEGV/panic) ? ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoiE 'eacces|permission|denied|<SEGV>|panic|cap' | sort | uniq -c | head
echo "=== log size ==="; wc -c "$LOG"
