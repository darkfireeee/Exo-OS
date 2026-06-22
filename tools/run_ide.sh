#!/bin/bash
# fast_iso (strip, pas de rebuild ExoPhoenix) + boot QEMU `pc` (IDE legacy) :
#   - ISO strippée en CD (boot)
#   - rootfs ExoFS en -hda (IDE primaire maître 0x1F0, lu par ata_pio)
# Teste le chemin ata_pio -> rootfs -> init -> #25 sur PC legacy (proxy Bochs).
source ~/.profile 2>/dev/null; export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
pkill -9 qemu-system-x86 2>/dev/null
bash tools/fast_iso.sh || { echo "FAST_ISO FAIL"; exit 1; }
LOG=/tmp/e9pcide.txt; rm -f "$LOG"
echo "=== QEMU pc + IDE (stripped ISO CD + rootfs -hda) 30s ==="
timeout 30 qemu-system-x86_64 -machine pc -m 256M -boot d -vga std -serial null -no-reboot -no-shutdown \
  -debugcon "file:$LOG" \
  -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
  -hda target/qemu/exofs-root.img \
  -cdrom exo-os.iso -display none 2>/dev/null
echo "=== markers (stage0 @ / IDE fallback #I#i / init / #25) ==="
tr -cd "\11\12\15\40-\176\n" < "$LOG" 2>/dev/null | grep -aoE "@[0-9a-e]|#I|#i|SECURITY|boot_display: [a-z ]+|init: [a-z_ ]+|INIT ELF|reached shell|SEGV pid=1" | tr "\n" " "
echo; echo "=== log size ==="; stat -c%s "$LOG" 2>/dev/null
