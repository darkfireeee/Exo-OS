#!/bin/bash
# Watchpoint sur node[0] (0xffff80000ff33040). A chaque ecriture: valeur+PC+pile.
# La pile (x/8a $rsp) contient l'adresse de retour vers le VRAI appelant (memset
# /memcpy n'ont pas de frame pointer). On resout ensuite l'ecriture corruptrice.
source ~/.profile 2>/dev/null; export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
pkill -9 -f qemu-system 2>/dev/null; sleep 1
cargo run -q -p exofs-mkroot -- --image target/qemu/exofs-root.img --size 536870912 --root target/exofs-rootfs 2>&1 | tail -1
LOG=/tmp/e9wp.txt; rm -f "$LOG"
timeout 70 qemu-system-x86_64 \
  -machine q35 -m 256M -boot d -vga std -serial null -no-reboot -no-shutdown \
  -debugcon "file:$LOG" \
  -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  -cdrom exo-os.iso -display none -s 2>/dev/null &
QPID=$!
sleep 7
ELF=target/exophoenix/kernel-a-debug.elf
cat > /tmp/gdbwp.txt <<'EOF'
target remote :1234
set pagination off
set $cnt = 0
watch *(unsigned long*)0xffff80000ff33040
commands
  set $cnt = $cnt + 1
  printf "\n##W%d val=%#lx PC=%#lx RSP=%#lx\n", $cnt, *(unsigned long*)0xffff80000ff33040, $pc, $rsp
  x/10a $rsp
  continue
end
continue
EOF
timeout 50 gdb -q -nx -batch -x /tmp/gdbwp.txt "$ELF" > /mnt/c/Users/xavie/Desktop/Exo-OS/tools/wpout.txt 2>&1
kill -9 $QPID 2>/dev/null; pkill -9 -f qemu-system 2>/dev/null
echo "=== nb ecritures ==="; grep -ac "##W" tools/wpout.txt
echo "=== resolve memset 0x3714d6 ==="; addr2line -f -C -e "$ELF" 0x3714d6 2>/dev/null | head -2
echo "=== TOUTES les ecritures (val + PC) ==="; grep -aE "##W" tools/wpout.txt
echo "=== progression ==="; tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '[a-z_]+: [a-z]+' | tail -3
