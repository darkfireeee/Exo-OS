set pagination off
set confirm off
set logging file target/qemu/gdb25_hits.log
set logging overwrite on
set logging redirect on
target remote :1234
hbreak *0x297400
continue
set $initcr3 = $cr3 & 0xFFFFFFFFFFFFF000
delete
set logging on
printf "init cr3 = 0x%lx\n", $initcr3
# Write-watchpoint on init stack slot (linear addr, follows CoW-breaks).
watch *(unsigned long long*)0x7ffffffefae8
commands
  silent
  printf "W pc=0x%lx cr3=0x%lx val=0x%lx\n", $pc, ($cr3 & 0xFFFFFFFFFFFFF000), *(unsigned long long*)0x7ffffffefae8
  continue
end
continue
