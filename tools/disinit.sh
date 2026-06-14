#!/bin/bash
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
B=target/exofs-rootfs/sbin/exo-init-server
echo "=== program headers (base vaddr) ==="
readelf -l "$B" 2>/dev/null | grep -A1 "LOAD" | head -10
echo "=== entry point ==="
readelf -h "$B" 2>/dev/null | grep -i entry
echo
echo "=== addr2line pour offset 0x1948e (RIP avant le saut NULL) ==="
addr2line -f -e "$B" 0x1948e 0x1000001948e 2>/dev/null
echo
echo "=== désassemblage autour de 0x1948e (le call/jmp NULL) ==="
objdump -d "$B" 2>/dev/null | grep -E "^\s+(1947[0-9a-f]|1948[0-9a-f]|1949[0-9a-f]):" | head -40
