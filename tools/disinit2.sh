#!/bin/bash
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
B=target/exofs-rootfs/sbin/exo-init-server
DBG=$(ls target/x86_64-exo-userspace/debug/deps/exo_init_server-* 2>/dev/null | grep -vE '\.(d|rmeta|o)$' | head -1)
echo "=== désassemblage rootfs autour de 0x1948e (vaddr 0x1000001948e) ==="
objdump -d "$B" 2>/dev/null | grep -E ':\s' | awk -F: '{a=strtonum("0x"$1)} a>=0x1000001940e && a<=0x100000195a0 {print}' | head -50
echo
echo "=== symbole le plus proche (debug build $DBG) ==="
if [ -n "$DBG" ]; then
  addr2line -f -e "$DBG" 0x1000001948e 2>/dev/null
  echo "--- nm symboles près de 0x1948e ---"
  nm "$DBG" 2>/dev/null | sort | awk '{a=strtonum("0x"$1)} a>=0x1000001900e && a<=0x100000196a0 {print}' | head
fi
