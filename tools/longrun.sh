#!/bin/bash
# Run-only, 60s, montre TOUS les marqueurs de progression dans l'ordre.
source ~/.profile 2>/dev/null; export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
pkill -9 -f qemu-system 2>/dev/null; sleep 1
cargo run -q -p exofs-mkroot -- --image target/qemu/exofs-root.img --size 536870912 --root target/exofs-rootfs 2>&1 | tail -1
LOG=tools/e9long.txt; rm -f "$LOG"
timeout 60 qemu-system-x86_64 \
  -machine q35 -m 256M -boot d -vga std -serial null -no-reboot -no-shutdown \
  -debugcon "file:$LOG" \
  -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  -cdrom exo-os.iso -display none 2>/dev/null
echo "=== marqueurs progression (ordre, hors BPG/MAP/INS) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | sed -E 's/<BPG[^>]*>//g; s/<MAP#[^>]*>//g; s/<INS[^>]*>//g' | grep -aoE '[a-z_]+: [a-zA-Z_0-9/ .-]+|p[0-9]s[0-9]+|<FORK[^>]*>|<EXEC[^>]*>|panic|PANIC' | tail -50
echo "=== syscalls PID2 (ipc_router) - les derniers ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE 'p2s[0-9]+' | tail -20 | tr '\n' ' '; echo
echo "=== log size ==="; wc -c "$LOG"
