#!/bin/bash
# Rebuild forcé (touch kernel) + test q35 (régression) ET pc+IDE (legacy PC + pilote ATA).
source ~/.profile 2>/dev/null; export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
pkill -9 qemu-system-x86 2>/dev/null; sleep 1
touch kernel/src/memory/physical/allocator/buddy.rs
echo "=== make iso (forcé) ==="
make iso >/tmp/mkrt.log 2>&1 && echo ISOOK || { echo BUILDFAIL; grep -iE "error\[|error:" /tmp/mkrt.log | head -15; exit 1; }
echo "=== q35 (virtio) régression ==="
rm -f /tmp/e9rt_q35.txt
timeout 18 qemu-system-x86_64 -machine q35 -m 256M -boot d -vga std -serial null -no-reboot -no-shutdown -debugcon "file:/tmp/e9rt_q35.txt" -device isa-debug-exit,iobase=0xf4,iosize=0x04 -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 -cdrom exo-os.iso -display none 2>/dev/null
echo "q35 E9 bytes: $(wc -c </tmp/e9rt_q35.txt 2>/dev/null)"
tr -cd "\11\12\15\40-\176" </tmp/e9rt_q35.txt 2>/dev/null | grep -aoE "init: (start|spawned) [a-z_]+|SEGV pid=1|reached shell" | tail -4
echo "=== pc + IDE (legacy PC + pilote ATA) ==="
rm -f /tmp/e9rt_pc.txt
timeout 22 qemu-system-x86_64 -machine pc -m 256M -boot d -vga std -serial null -no-reboot -no-shutdown -debugcon "file:/tmp/e9rt_pc.txt" -device isa-debug-exit,iobase=0xf4,iosize=0x04 -hda target/qemu/exofs-root.img -cdrom exo-os.iso -display none 2>/dev/null
echo "pc E9 bytes: $(wc -c </tmp/e9rt_pc.txt 2>/dev/null)"
tr -cd "\11\12\15\40-\176" </tmp/e9rt_pc.txt 2>/dev/null | grep -aoE "init: (start|spawned) [a-z_]+|#i|#I|SEGV pid=1|reached shell" | tail -5
echo "=== DONE ==="
