#!/bin/bash
# Boot-test long (150s, sans rebuild) : jusqu'où va le boot après les changements cap ?
source ~/.profile 2>/dev/null; export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
pkill -9 -f qemu-system 2>/dev/null; sleep 1
LOG=tools/e9bt2.txt; rm -f "$LOG"
timeout 150 qemu-system-x86_64 \
  -machine q35 -m 256M -boot d -vga std -serial null -no-reboot -no-shutdown \
  -debugcon "file:$LOG" \
  -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  -cdrom exo-os.iso -display none 2>/dev/null
echo "=== progression boot (servers/waves/shell) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE 'init: (start|spawned|ready|timeout|cwave) [a-z_]+|[a-z0-9_]+: (boot|registered|ready)|<EXEC [^>]*>|exosh[^<]*|ExoSH' | tail -40
echo "=== serveurs lancés (EXEC uniq) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<EXEC [^>]*>' | sort | uniq -c
echo "=== anomalies ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoiE 'eacces|denied|<SEGV>|panic' | sort | uniq -c | head
echo "=== log size ==="; wc -c "$LOG"
