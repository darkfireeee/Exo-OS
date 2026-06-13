#!/bin/bash
source ~/.profile 2>/dev/null; export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
ELF=target/exophoenix/kernel-a-debug.elf
echo "=== fn contenant 0x31fca0 (2e IP fault) ==="
addr2line -f -C -e "$ELF" 0x31fca0 0x31fc00 0x31fc50
echo "=== nm trié autour de 0x31fca0 ==="
nm -nC "$ELF" | grep -iE '^0031f' | head -25
echo "=== fn contenant 0x1df831 (blake3) ==="
addr2line -f -C -e "$ELF" 0x1df831
echo "=== callers blake3 dans exoledger / audit ? symboles exoledger ==="
nm -C "$ELF" | grep -iE 'exoledger|append_audit|audit.*hash|ledger' | head -15
