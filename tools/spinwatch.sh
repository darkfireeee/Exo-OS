#!/bin/bash
# Attache TOT, casse a BlobCache::insert, dump l'etat du BTreeMap a chaque insert
# AVANT que le memmove geant (0x80000000 elts) ne detruise la memoire.
# Compare insert#1 (map vide) et insert#2 (1 entree -> celui qui spin).
source ~/.profile 2>/dev/null; export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
pkill -9 -f qemu-system 2>/dev/null; sleep 1
cargo run -q -p exofs-mkroot -- --image target/qemu/exofs-root.img --size 536870912 --root target/exofs-rootfs 2>&1 | tail -1
LOG=/tmp/e9w.txt; rm -f "$LOG"
timeout 75 qemu-system-x86_64 \
  -machine q35 -m 256M -boot d -vga std -serial null -no-reboot -no-shutdown \
  -debugcon "file:$LOG" \
  -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  -cdrom exo-os.iso -display none -s 2>/dev/null &
QPID=$!
sleep 6
ELF=target/exophoenix/kernel-a-debug.elf
cat > /tmp/gdbw.txt <<'EOF'
target remote :1234
set pagination off
define dumpmap
  printf "  root.height=%#lx root.node=%#lx length=%#lx\n", *(long*)0x590530, *(long*)0x590538, *(long*)0x590540
  set $np = *(unsigned long*)0x590538
  if $np > 0x1000
    printf "  ROOT NODE @%#lx (parent,paridx/len,keys..):\n", $np
    x/8gx $np
  end
end
hbreak *0x127770
commands
  silent
  printf ">>> INSERT entry\n"
  dumpmap
  continue
end
printf "=== run (hbreak BlobCache::insert) ===\n"
continue
EOF
timeout 50 gdb -q -nx -batch -x /tmp/gdbw.txt "$ELF" > /tmp/wout.txt 2>&1
kill -9 $QPID 2>/dev/null; pkill -9 -f qemu-system 2>/dev/null
echo "===== ETATS ====="
grep -aE 'INSERT|root\.|ROOT NODE|0x[0-9a-f]+ <|0x[0-9a-f]+:|attente|step' /tmp/wout.txt | head -50
echo "===== progression ====="; tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '[a-z_]+: [a-z]+|<INS [^>]*>' | tail -6
