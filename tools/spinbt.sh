#!/bin/bash
# Halte le CPU EN PLEIN SPIN (pas au 1er insert) et remonte la pile.
# Objectif : trouver le VRAI appelant du BTreeMap::insert qui boucle,
# que l'ICF masque au niveau symbole.
source ~/.profile 2>/dev/null; export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
pkill -9 -f qemu-system 2>/dev/null; sleep 1
echo "=== touch + rebuild iso (frame pointers) ==="
find kernel/src -name '*.rs' -exec touch {} + 2>/dev/null
touch .cargo/config.toml
make iso >/tmp/mk.log 2>&1 && echo "iso ok" || { echo "BUILD FAIL"; grep -iE 'error' /tmp/mk.log | tail -8; exit 1; }
cargo run -q -p exofs-mkroot -- --image target/qemu/exofs-root.img --size 536870912 --root target/exofs-rootfs 2>&1 | tail -1
LOG=/tmp/e9bt.txt; rm -f "$LOG"
timeout 60 qemu-system-x86_64 \
  -machine q35 -m 256M -boot d -vga std -serial null -no-reboot -no-shutdown \
  -debugcon "file:$LOG" \
  -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  -cdrom exo-os.iso -display none -s 2>/dev/null &
QPID=$!
# Laisser le système atteindre le spin
sleep 28
ELF=target/exophoenix/kernel-a-debug.elf
cat > /tmp/gdbbt.txt <<'EOF'
target remote :1234
echo \n=== PC / regs ===\n
printf "RIP=%#lx RSP=%#lx RBP=%#lx CR3=%#lx\n", $pc, $rsp, $rbp, $cr3
echo \n=== fonction au PC ===\n
info symbol $pc
echo \n=== desassemblage de la fonction courante (boucle reelle) ===\n
disas
echo \n=== BACKTRACE (frame pointers actifs) ===\n
bt 40
echo \n=== re-echantillon : 3 halts pour voir si le PC bouge (boucle large?) ===\n
stepi 200
printf "after 200 stepi: RIP=%#lx\n", $pc
info symbol $pc
stepi 2000
printf "after 2200 stepi: RIP=%#lx\n", $pc
info symbol $pc
EOF
timeout 40 gdb -q -nx -batch -x /tmp/gdbbt.txt "$ELF" 2>&1 | tee /tmp/btout.txt | grep -aE '===|RIP=|after|#[0-9]|in section|0x[0-9a-f]+ <|exo_os_kernel' | head -200
kill -9 $QPID 2>/dev/null; pkill -9 -f qemu-system 2>/dev/null
echo "=== progression serveurs ==="; tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '[a-z_]+: [a-z]+' | tail -5
