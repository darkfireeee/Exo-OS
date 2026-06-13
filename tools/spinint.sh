#!/bin/bash
# -d int sur le build courant (frame pointers) : trouve l'IP cpl=0 DOMINANT
# (statistique, fiable) et le resout en source. Determine si le spin est dans
# .text (code legitime qui boucle) ou .data (execution sauvage).
source ~/.profile 2>/dev/null; export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
pkill -9 -f qemu-system 2>/dev/null; sleep 1
cargo run -q -p exofs-mkroot -- --image target/qemu/exofs-root.img --size 536870912 --root target/exofs-rootfs 2>&1 | tail -1
INT=/tmp/qint2.log; LOG=/tmp/e9i2.txt; rm -f "$INT" "$LOG"
timeout 22 qemu-system-x86_64 \
  -machine q35 -m 256M -boot d -vga std -serial null -no-reboot -no-shutdown \
  -d int,cpu_reset -D "$INT" -debugcon "file:$LOG" \
  -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  -cdrom exo-os.iso -display none 2>/dev/null
echo "=== distribution cpl sur v=20 (timer) ==="
grep -aE 'v=20 ' "$INT" | grep -aoE 'cpl=[0-9]' | sort | uniq -c
echo "=== TOP 8 IP cpl=0 (le spin kernel) ==="
grep -aE 'v=20 .*cpl=0 ' "$INT" | grep -aoE 'IP=[0-9a-f]+:[0-9a-f]+' | sort | uniq -c | sort -rn | head -8
echo "=== TOP 5 IP cpl=3 (userspace) ==="
grep -aE 'v=20 .*cpl=3 ' "$INT" | grep -aoE 'IP=[0-9a-f]+:[0-9a-f]+' | sort | uniq -c | sort -rn | head -5
echo "=== vecteurs d'exception (histogramme) ==="
grep -aoE 'v=[0-9a-f]{2} ' "$INT" | sort | uniq -c | sort -rn | head
echo "=== y a-t-il des #PF KERNEL (cpl=0 v=0e) ? ==="
grep -aE 'v=0e .*cpl=0 ' "$INT" | grep -aoE 'IP=[0-9a-f:]+ .*CR2=[0-9a-f]+' | sort | uniq -c | sort -rn | head -5
echo "=== addr2line des TOP IP cpl=0 ==="
for ip in $(grep -aE 'v=20 .*cpl=0 ' "$INT" | grep -aoE 'IP=[0-9a-f]+:[0-9a-f]+' | sed -E 's/.*://' | sort | uniq -c | sort -rn | head -8 | grep -aoE '[0-9a-f]+$'); do
  printf "0x%s -> " "$ip"
  addr2line -f -C -e target/exophoenix/kernel-a-debug.elf "0x$ip" 2>/dev/null | tr '\n' ' '
  echo
done
echo "=== progression e9 ==="; tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '[a-z_]+: [a-z]+' | tail -6
