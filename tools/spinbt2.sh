#!/bin/bash
# Boot (iso deja construit avec frame pointers) + halt en plein spin + bt propre.
source ~/.profile 2>/dev/null; export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
pkill -9 -f qemu-system 2>/dev/null; sleep 1
cargo run -q -p exofs-mkroot -- --image target/qemu/exofs-root.img --size 536870912 --root target/exofs-rootfs 2>&1 | tail -1
LOG=/tmp/e9bt.txt; rm -f "$LOG"
timeout 55 qemu-system-x86_64 \
  -machine q35 -m 256M -boot d -vga std -serial null -no-reboot -no-shutdown \
  -debugcon "file:$LOG" \
  -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  -cdrom exo-os.iso -display none -s 2>/dev/null &
QPID=$!
sleep 28
ELF=target/exophoenix/kernel-a-debug.elf
cat > /tmp/gdbbt2.txt <<'EOF'
target remote :1234
printf "PC0 = %#lx\n", $pc
info symbol $pc
echo === BACKTRACE ===\n
bt 50
echo === apres 5000 stepi ===\n
stepi 5000
printf "PC1 = %#lx\n", $pc
info symbol $pc
bt 12
EOF
timeout 35 gdb -q -nx -batch -x /tmp/gdbbt2.txt "$ELF" > /tmp/btclean.txt 2>&1
kill -9 $QPID 2>/dev/null; pkill -9 -f qemu-system 2>/dev/null
echo "===== BT CLEAN ====="
grep -aE 'PC[0-9]|=== |#[0-9]+ |is in| in section' /tmp/btclean.txt | head -80
