#!/bin/bash
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
B=target/exofs-rootfs/sbin/exo-init-server
objdump -d "$B" 2>/dev/null > /tmp/initdis.txt
echo "=== autour de 4487 (return addr) — l'instruction avant = le call ==="
grep -nE '^\s+44[0-9a-f][0-9a-f]:' /tmp/initdis.txt | awk -F: '{n=strtonum("0x"$2)} n>=0x4470 && n<=0x4495'
echo "=== autour de 515f ==="
grep -nE '^\s+51[0-9a-f][0-9a-f]:' /tmp/initdis.txt | awk -F: '{n=strtonum("0x"$2)} n>=0x5148 && n<=0x5165'
echo "=== tous les call/jmp indirects de init (NULL fn ptr possible) ==="
grep -E 'call\s+\*|jmp\s+\*|ff /[0-9]' /tmp/initdis.txt | head -20
