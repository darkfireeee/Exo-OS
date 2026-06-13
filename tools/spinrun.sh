#!/bin/bash
source ~/.profile 2>/dev/null; export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
pkill -9 -f qemu-system 2>/dev/null; sleep 1
cargo run -q -p exofs-mkroot -- --image target/qemu/exofs-root.img --size 536870912 --root target/exofs-rootfs 2>&1 | tail -1
LOG=/tmp/e9g.txt; rm -f "$LOG"
timeout 55 qemu-system-x86_64 \
  -machine q35 -m 256M -boot d -vga std -serial null -no-reboot -no-shutdown \
  -debugcon "file:$LOG" \
  -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  -cdrom exo-os.iso -display none -s 2>/dev/null &
QPID=$!
sleep 24
cat > /tmp/gdbcmd.txt <<'EOF'
target remote :1234
break node.rs:1240
continue
echo \n=== BT (frame pointers) ===\n
bt 50
EOF
timeout 25 gdb -q -nx -batch -x /tmp/gdbcmd.txt target/exophoenix/kernel-a-debug.elf 2>&1 | grep -aE '^#[0-9]|=== |exo_os_kernel|Breakpoint [0-9]+,' | head -50
kill -9 $QPID 2>/dev/null
echo "=== e9 ==="; tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE 'ipc_router: [a-z]+' | tail -3
