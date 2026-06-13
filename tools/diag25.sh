#!/bin/bash
source ~/.profile 2>/dev/null; export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
pkill -9 -f qemu-system 2>/dev/null; sleep 1
touch kernel/src/syscall/table.rs kernel/src/syscall/fs_bridge.rs kernel/src/syscall/handlers/signal.rs
make iso >/tmp/mk.log 2>&1 && echo "ISOOK" || { echo "BUILDFAIL"; grep -iE "error\[|error:" /tmp/mk.log | head -15; exit 1; }
cargo run -q -p exofs-mkroot -- --image target/qemu/exofs-root.img --size 536870912 --root target/exofs-rootfs 2>&1 | tail -1
LOG=tools/e9d25.txt; rm -f "$LOG"
timeout 55 qemu-system-x86_64 \
  -machine q35 -m 256M -boot d -vga std -serial null -no-reboot -no-shutdown \
  -debugcon "file:$LOG" \
  -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  -cdrom exo-os.iso -display none 2>/dev/null
echo "=== CRT (registrations endpoint) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<CRT [^>]*>' | sort | uniq -c
echo "=== LKP (lookups init readiness) — uniques + comptes ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<LKP [^>]*>' | sort | uniq -c | head -20
echo "=== CONSOLE (logs init/shell via miroir E9) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE 'init[_a-z]*: [a-zA-Z_/ =0-9-]+|exosh[^ ]*|\$ |# ' | head -40
echo "=== K0 (pid_alive : init check existence process) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<K0 [^>]*>' | sort | uniq -c | head
echo "=== EXEC serveurs ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<EXEC [^>]*>' | sort | uniq -c
echo "=== log size ==="; wc -c "$LOG"
