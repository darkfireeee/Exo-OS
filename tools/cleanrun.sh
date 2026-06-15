#!/bin/bash
# Run propre post-nettoyage : build + QEMU 150s + observation des logs init/serveurs
# permanents (les marqueurs E9 temporaires ont été retirés). Timer obligatoire.
source ~/.profile 2>/dev/null; export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
pkill -9 -f qemu-system 2>/dev/null; sleep 1
# Touch tous les fichiers édités (mtime Windows->WSL) pour forcer la recompilation.
touch kernel/src/arch/x86_64/exceptions.rs kernel/src/arch/x86_64/syscall.rs \
  kernel/src/scheduler/core/switch.rs kernel/src/syscall/fast_path.rs \
  kernel/src/fs/exofs/storage/virtio_adapter.rs servers/init_server/src/*.rs
make iso >/tmp/mk.log 2>&1 && echo "ISOOK" || { echo "BUILDFAIL"; grep -iE "error\[|error:" /tmp/mk.log | head -20; exit 1; }
cargo run -q -p exofs-mkroot -- --image target/qemu/exofs-root.img --size 536870912 --root target/exofs-rootfs 2>&1 | tail -1
LOG=tools/e9clean.txt; rm -f "$LOG"
timeout 150 qemu-system-x86_64 \
  -machine q35 -m 256M -boot d -vga std -serial null -no-reboot -no-shutdown \
  -debugcon "file:$LOG" \
  -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  -cdrom exo-os.iso -display none 2>/dev/null
echo "=== progression boot (init/serveurs/shell, dans l ordre) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE 'init: [a-zA-Z_/, =0-9-]+|[a-z0-9_]+: (boot|registered|ready|started)|<EXEC [^>]*>|exosh[^<]*|ExoSH[^<]*|\$' | tail -50
echo "=== serveurs lancés (EXEC uniq) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<EXEC [^>]*>' | sort | uniq -c
echo "=== fautes non résolues (SEGV/OOM) ? ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<SEGV>|<POOM>|PANIC|panic' | sort | uniq -c | head
echo "=== log size ==="; wc -c "$LOG"
