set pagination off
set confirm off
set logging file target/qemu/gdb25_hits.log
set logging overwrite on
set logging redirect on
target remote :1234
hbreak *0x297400
continue
set logging on
set $initcr3 = $cr3
set language c
set $pm = 0xFFFF800000000000
set $m = 0xFFFFFFFFF000
set $e4 = *(long*)($pm + $initcr3 + 0x7f8)
set $e3 = *(long*)($pm + ($e4 & $m) + 0xff8)
set $e2 = *(long*)($pm + ($e3 & $m) + 0xff8)
set $pteaddr = $pm + ($e2 & $m) + 0xf78
printf "init cr3=0x%lx stack PTE@0x%lx=0x%lx\n", $initcr3, $pteaddr, *(long*)$pteaddr
delete
watch *(long*)$pteaddr if (*(long*)$pteaddr & 2) != 0
continue
set $fprime = *(long*)$pteaddr & $m
set $fidx = $fprime >> 12
printf "\n=== CoW-break: init stack F'=0x%lx idx=0x%lx ===\n", $fprime, $fidx
delete
# HARDWARE breakpoints (work under KPTI; software 'break' does not) on free_pages,
# conditioned on the freed frame == init's stack frame F'. If F' is freed -> reused
# -> corruption. Check rsi AND rdi for both phys and index representations.
hbreak *0x1ec730 if ($rsi == $fprime) || ($rsi == $fidx) || ($rdi == $fprime) || ($rdi == $fidx)
hbreak *0x1f92e0 if ($rsi == $fprime) || ($rsi == $fidx) || ($rdi == $fprime) || ($rdi == $fidx)
# Backup: physmap write to F'+0xae8 (the corrupted slot).
watch *(long*)($pm + $fprime + 0xae8)
continue
printf "\n*** HIT: init stack frame F' freed/written ***\n"
printf "pc=0x%lx cr3=0x%lx rsi=0x%lx rdi=0x%lx rdx=0x%lx\n", $pc, $cr3, $rsi, $rdi, $rdx
echo --- BACKTRACE (the culprit) ---\n
bt 18
echo --- instructions ---\n
x/5i $pc
