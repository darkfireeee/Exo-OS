set pagination off
set confirm off
set logging file /tmp/gdb25pc.log
set logging overwrite on
set logging redirect on
target remote :1234
set language c
set logging on
watch *(long*)0xFFFF80000582dae8
commands
  silent
  printf "\n==== W pc=0x%lx cr3=0x%lx val=0x%lx ====\n", $pc, $cr3, *(long*)0xFFFF80000582dae8
  bt 18
  continue
end
continue
