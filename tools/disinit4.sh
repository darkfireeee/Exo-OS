#!/bin/bash
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
B=target/exofs-rootfs/sbin/exo-init-server
[ -f /tmp/initdis.txt ] || objdump -d "$B" 2>/dev/null > /tmp/initdis.txt
echo "=== instructions autour de 0x4487 (la call qui retourne ici) ==="
grep -nE '10000004(46|47|48)[0-9a-f]:' /tmp/initdis.txt | head -25
echo "=== autour de 0x515f ==="
grep -nE '1000000515[0-9a-f]:|1000000514[0-9a-f]:' /tmp/initdis.txt | head -20
echo "=== nom de fonction contenant 0x4487 et 0x515f ==="
awk '/^[0-9a-f]+ <.*>:/{f=$0} /10000004487:/{print "4487 dans:", f} /1000000515f:/{print "515f dans:", f}' /tmp/initdis.txt
