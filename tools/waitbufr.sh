#!/bin/bash
F="/mnt/c/Users/xavie/AppData/Local/Temp/claude/C--Users-xavie-Desktop-Exo-OS/a9d46684-607e-4afa-a39d-9299bf8585b1/tasks/bufr5efc5.output"
n=0
while [ "$n" -lt 175 ]; do
  if grep -qaE "log size|BUILDFAIL|error\[" "$F" 2>/dev/null; then break; fi
  sleep 3
  n=$((n + 1))
done
if grep -qaE "BUILDFAIL|error\[" "$F" 2>/dev/null; then
  echo "BUILD FAILED:"; grep -aE "error\[|BUILDFAIL" "$F" | head -5; exit 1
fi
# build ok ; run longer (180s) on the current iso
source ~/.profile 2>/dev/null; export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
pkill -9 -f qemu-system 2>/dev/null; sleep 1
cargo run -q -p exofs-mkroot -- --image target/qemu/exofs-root.img --size 536870912 --root target/exofs-rootfs 2>&1 | tail -1
LOG=tools/e9rip.txt; rm -f "$LOG"
timeout 180 qemu-system-x86_64 \
  -machine q35 -m 256M -boot d -vga std -serial null -no-reboot -no-shutdown \
  -debugcon "file:$LOG" \
  -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  -cdrom exo-os.iso -display none 2>/dev/null
C() { tr -cd '\11\12\15\40-\176' < "$LOG"; }
echo "=== RIPCORR (schedule) + RIPSIG (signal) ==="
C | grep -aoE '<RIPCORR before=[0-9a-f]+ after=[0-9a-f]+>|<RIPSIG before=[0-9a-f]+ after=[0-9a-f]+>' | head -8
echo "RIPCORR=$(C | grep -aoc '<RIPCORR') RIPSIG=$(C | grep -aoc '<RIPSIG')"
echo "=== crash ==="
C | grep -aoE 'init: spawnret [a-z_]+ pid=[0-9]+|init: pr[0-9]-[a-z-]+|init: monoC|<CGwOK>|<RIPCORR[^>]*>|<RIPSIG[^>]*>|<SEGV>' | tail -10
echo "log: $(wc -c < "$LOG")"
