#!/bin/bash
source ~/.bashrc 2>/dev/null
source ~/.profile 2>/dev/null
export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
pkill -9 -f qemu-system 2>/dev/null
sleep 1
echo "=== touch sources éditées (fix mtime Windows->WSL) ==="
find kernel/src -name '*.rs' -exec touch {} + 2>/dev/null
echo "=== rebuild iso ==="
make iso >/tmp/mk.log 2>&1 && echo "iso ok" || { echo "BUILD FAIL"; grep -iE 'error' /tmp/mk.log | tail -5; exit 1; }
echo "=== regen disque frais ==="
cargo run -q -p exofs-mkroot -- --image target/qemu/exofs-root.img --size 536870912 --root target/exofs-rootfs 2>&1 | tail -1
LOG=/tmp/e9p.txt; ERR=/tmp/qerr.txt; INT=/tmp/qint.log
rm -f "$LOG" "$ERR" "$INT"
echo "=== run QEMU (disque frais, flags canoniques) ==="
timeout 28 qemu-system-x86_64 \
  -machine q35 -m 256M -boot d -vga std -serial null \
  -no-reboot -no-shutdown \
  -d int,cpu_reset -D "$INT" \
  -debugcon "file:$LOG" \
  -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
  -drive if=none,file=target/qemu/exofs-root.img,format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  -cdrom exo-os.iso -display none 2>"$ERR"
echo "=== e9 nettoyé (queue) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | sed 's/boot_display: //g; s/stage ok /\nSTAGEOK:/g' | tail -c 250
echo ""
echo "=== séquence syscalls PID1+PID2 (p<pid>s<nr>) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE ' p[12]s[0-9]+' | tr '\n' ' ' | tail -c 900; echo ""
echo "--- derniers syscalls PID2 (ipc_router) ---"
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE ' p2s[0-9]+' | tail -15 | tr '\n' ' '; echo ""
echo "--- histogramme syscalls ---"
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE ' p[12]s[0-9]+' | sort | uniq -c | sort -rn | head -15
echo "=== gates qui bloquent fork/execve (PID1) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<(AUDITeperm|AUDITenosys|ZTDENY|SLOWPATH) nr=[0-9]+( eff=[0-9]+)?>' | sort | uniq -c | head -20
echo "=== FORK / EXEC (si atteints) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<FORK p=[0-9]>(=OK c=|=ER)?' | head -8
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<EXEC [^>]{1,48}>(=OK|=ER:[A-Za-z]+)?' | head -8
echo "=== #PF uniq (cr2/rip distincts vs répétés) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '#PF cr2=[0-9a-f]+ rip=[0-9a-f]+' | sort | uniq -c | sort -rn | head -20
echo "=== total #PF ==="; tr -cd '\11\12\15\40-\176' < "$LOG" | grep -ac '#PF cr2='
echo "=== séquence vecteurs exceptions (v=XX) depuis -d int ==="
grep -aoE 'v=[0-9a-f]+' "$INT" 2>/dev/null | sort | uniq -c | sort -rn | head
echo "--- 1ères transitions check_exception/excp ---"
grep -aoE 'check_exception old: 0x[0-9a-f]+ new 0x[0-9a-f]+' "$INT" 2>/dev/null | head -8
echo "=== où tourne-t-il ? (cpl/IP des timer v=20, échantillon fin de log) ==="
grep -aoE 'v=20 e=[0-9a-f]+ i=[0-9] cpl=[0-9] IP=[0-9a-f]+:[0-9a-f]+' "$INT" 2>/dev/null | tail -5
echo "--- distribution cpl sur v=20 ---"
grep -aoE 'v=20 [^I]*cpl=[0-9]' "$INT" 2>/dev/null | grep -aoE 'cpl=[0-9]' | sort | uniq -c
echo "--- dernier bloc exception complet (v=0e ou autre) ---"
grep -aE 'v=0e|v=0d|v=08|v=06' "$INT" 2>/dev/null | tail -3
echo "=== 1er fault KERNEL (cpl=0) — frame registres complet ==="
awk '/v=0e e=000[0-9] i=0 cpl=0/{f=1} f{print; n++} n>=30{exit}' "$INT" 2>/dev/null | head -30
echo "=== REDZONE débordements heap (taille de l'alloc fautive) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<REDZ overflow user_size=[0-9]+>' | sort | uniq -c | head
echo "=== CALLERS insert/write_at (DERNIER = spin) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<INS [^>]+>|<WR [^>]+>' | tail -14
echo "--- histogramme ---"
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<INS [^>]+>|<WR [^>]+>' | sed 's#.*/##' | sort | uniq -c | sort -rn | head
echo "=== #UD userspace (serveur qui panique) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<UD rip=0x[0-9a-f]+>' | sort | uniq -c | head
echo "=== e9 progression (serveurs démarrés) ==="
tr -cd '\11\12\15\40-\176' < "$LOG" | grep -aoE '<EXEC [^>]{1,40}>(=OK|=ER:[A-Za-z]+)?|[a-z_]+: (boot|registered|ready|started)|exosh' | tail -25
echo "=== sizes ==="; wc -c "$LOG" "$INT"
