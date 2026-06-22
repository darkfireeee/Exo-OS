#!/bin/bash
# Build ISO RAPIDE pour itération : NE recompile PAS Kernel A (ExoPhoenix release+LTO,
# ~10 min). Réutilise l'image kernel-a-debug.elf existante et ne reconstruit que le
# Kernel B (debug). Rootfs réutilisé tel quel (passer REBUILD_ROOTFS=1 pour le refaire).
source ~/.profile 2>/dev/null; export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
KA="$(pwd)/target/exophoenix/kernel-a-debug.elf"
if [ ! -f "$KA" ]; then echo "FATAL: pas de $KA — lancer 'make iso' une fois d'abord"; exit 1; fi
echo "=== build Kernel B seul (réutilise Kernel A, saute ExoPhoenix) ==="
( cd kernel && KERNEL_A_IMAGE_PATH="$KA" cargo build --target x86_64-unknown-none -Z build-std=core,alloc,compiler_builtins -Z build-std-features=compiler-builtins-mem ) >/tmp/fastb.log 2>&1 || { echo BUILDFAIL; grep -iE "error\[|error:|cannot find|no method|unresolved" /tmp/fastb.log | head -20; exit 1; }
cargo run -q -p exo-kernel-signer -- sign target/x86_64-unknown-none/debug/exo-os-kernel >/dev/null 2>&1 || echo "(sign ignoré)"
if [ "$REBUILD_ROOTFS" = "1" ]; then echo "=== rootfs ==="; make rootfs-image >/tmp/fastroot.log 2>&1 && echo "rootfs OK" || echo "rootfs FAIL"; fi
echo "=== ISO ==="
rm -rf iso_build; mkdir -p iso_build/boot/grub
cp target/x86_64-unknown-none/debug/exo-os-kernel iso_build/boot/exo-os-kernel
# FIX-BOOT-LEGACY : strip pour rendre le kernel bootable sur BIOS legacy (cf Makefile _make_iso).
strip --strip-all iso_build/boot/exo-os-kernel 2>/dev/null || objcopy --strip-all iso_build/boot/exo-os-kernel 2>/dev/null || true
cp bootloader/grub.cfg iso_build/boot/grub/grub.cfg
grub-mkrescue -o exo-os.iso iso_build --compress=xz 2>&1 | grep -iE "error|produced|completed" | tail -1
rm -rf iso_build
echo "FASTISO done: $(stat -c%s exo-os.iso 2>/dev/null) bytes"
