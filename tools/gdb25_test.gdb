set pagination off
set confirm off
target remote :1234
echo \n=== connected ===\n
hbreak *0x297400
continue
echo \n=== HIT do_fork (init forks) ===\n
info registers rip rsp
printf "cr3=0x%lx\n", $cr3
printf "cr2=0x%lx\n", $cr2
x/3i $pc
detach
quit
