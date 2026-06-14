#!/bin/bash
source ~/.profile 2>/dev/null; export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
pkill -9 -f qemu-system 2>/dev/null; sleep 1
touch kernel/src/process/lifecycle/fork.rs servers/init_server/src/*.rs kernel/src/arch/x86_64/exceptions.rs kernel/src/memory/virtual/fault/handler.rs kernel/src/arch/x86_64/spectre/kpti.rs kernel/src/memory/virtual/page_table/kpti_split.rs kernel/src/scheduler/core/switch.rs \
  kernel/src/process/signal/handler.rs kernel/src/memory/virtual/fault/cow.rs kernel/src/memory/virtual/fault/demand_paging.rs kernel/src/memory/physical/allocator/buddy.rs kernel/src/memory/virtual/address_space/user.rs kernel/src/memory/virtual/page_table/walker.rs kernel/src/fs/exofs/storage/virtio_adapter.rs
make iso >/tmp/mk.log 2>&1 && echo "ISOOK" || { echo "BUILDFAIL"; grep -iE "error\[|error:" /tmp/mk.log | head -15; exit 1; }
cargo run -q -p exofs-mkroot -- --image target/qemu/exofs-root.img --size 536870912 --root target/exofs-rootfs 2>&1 | tail -1
LOG=tools/e9d25b.txt; rm -f "$LOG"
timeout 50 qemu-system-x86_64 \
  -machine q35 -m 256M -boot d -vga std -serial null -no-reboot -no-shutdown \
  -debugcon "file:$LOG" \
  -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  -cdrom exo-os.iso -display none 2>/dev/null
echo "=== CONSOLE (logs init/serveurs/shell via miroir E9, dans l ordre) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE 'init[_a-z]*: [a-zA-Z_/, =0-9-]+|[a-z_]+_server: [a-zA-Z ]+|exosh[^<]*|ExoSH[^<]*|\$ ' | head -60
echo "=== serveurs lancés (EXEC) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<EXEC [^>]*>' | sort | uniq -c
echo "=== K0 / LKP / EXEC / SEGV ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<K0 [^>]*>|<LKP [^>]*>|<EXEC [^>]*>|<SEGV>|<POOM>' | sort | uniq -c | head
echo "=== SW (transitions CR3 scheduler: prev->next, cr3 chargé) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<SW [^>]*>' | head -40
echo "=== CR3 (as=handler tk=kernel ... present) — uniq ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<CR3 [^>]*>' | sort | uniq -c | head -8
echo "=== XF (faute exec : present / vma EXEC / page_flags NX) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<XF [^>]*>' | head -14
echo "=== fautes userspace (PF) derniers ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<PF p[0-9]+ a=[0-9a-f]+ [RWX]>' | tail -10
echo "=== log size ==="; wc -c "$LOG"
