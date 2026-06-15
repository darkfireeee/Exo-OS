#!/bin/bash
# Probe SR : où init se bloque entre la vague et service_ready (sr1/sr2/sr3).
source ~/.profile 2>/dev/null; export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
pkill -9 -f qemu-system 2>/dev/null; sleep 1
touch servers/init_server/src/*.rs
make iso >/tmp/mk.log 2>&1 && echo "ISOOK" || { echo "BUILDFAIL"; grep -iE "error\[|error:" /tmp/mk.log | head -20; exit 1; }
cargo run -q -p exofs-mkroot -- --image target/qemu/exofs-root.img --size 536870912 --root target/exofs-rootfs 2>&1 | tail -1
LOG=tools/e9sr.txt; rm -f "$LOG"
timeout 60 qemu-system-x86_64 \
  -machine q35 -m 256M -boot d -vga std -serial null -no-reboot -no-shutdown \
  -debugcon "file:$LOG" \
  -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  -cdrom exo-os.iso -display none 2>/dev/null
echo "=== séquence init (wl-done / sr1 / sr2 / sr3 result) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE 'init: (start|spawned|wl-done|sr1|sr2|sr3 [a-z_]+ [A-Za-z,+]+|ready|timeout) ?[a-z_0-9 =]*' | tail -30
echo "=== compteurs ==="
echo "wl-done: $(tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoc 'init: wl-done')"
echo "sr1: $(tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoc 'init: sr1')   sr2: $(tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoc 'init: sr2')"
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE 'init: sr3 [a-z_]+ [A-Za-z,+]+' | sort | uniq -c
echo "=== log size ==="; wc -c "$LOG"
