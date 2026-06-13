#!/bin/bash
# Breakpoint EXACT sur ptr::copy::<MaybeUninit<BlobId>> (0x1476c3) + bt profond
# (frame pointers actifs) => revele le site d'appel exact qui corrompt/spin.
source ~/.profile 2>/dev/null; export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
pkill -9 -f qemu-system 2>/dev/null; sleep 1
cargo run -q -p exofs-mkroot -- --image target/qemu/exofs-root.img --size 536870912 --root target/exofs-rootfs 2>&1 | tail -1
LOG=/tmp/e9bt3.txt; rm -f "$LOG"
timeout 55 qemu-system-x86_64 \
  -machine q35 -m 256M -boot d -vga std -serial null -no-reboot -no-shutdown \
  -debugcon "file:$LOG" \
  -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  -cdrom exo-os.iso -display none -s 2>/dev/null &
QPID=$!
sleep 27
ELF=target/exophoenix/kernel-a-debug.elf
cat > /tmp/gdbbt3.txt <<'EOF'
target remote :1234
echo === halt initial ===\n
printf "PC=%#lx\n", $pc
break *0x1476c3
continue
echo === DANS ptr::copy<MaybeUninit<BlobId>> ===\n
printf "PC=%#lx RSI(src)=%#lx RDI(dst)=%#lx RCX/len=%#lx RDX=%#lx\n", $pc, $rsi, $rdi, $rcx, $rdx
echo === BACKTRACE PROFOND ===\n
bt 60
EOF
timeout 30 gdb -q -nx -batch -x /tmp/gdbbt3.txt "$ELF" > /tmp/btclean3.txt 2>&1
kill -9 $QPID 2>/dev/null; pkill -9 -f qemu-system 2>/dev/null
echo "===== RESULTAT ====="
grep -aE 'PC=|RSI|=== |#[0-9]+ ' /tmp/btclean3.txt | head -80
