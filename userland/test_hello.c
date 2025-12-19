// Simple hello program for Exo-OS exec() testing
// No libc dependencies - raw syscalls only

void _start() {
    // Write "Hello from exec!\n" to stdout (fd=1)
    const char msg[] = "Hello from exec!\n";
    unsigned long msg_len = 17;
    
    // Syscall: write(fd=1, buf=msg, count=17)
    // SYS_write = 1
    __asm__ volatile(
        "mov $1, %%rax\n"      // syscall number: write
        "mov $1, %%rdi\n"      // fd = 1 (stdout)
        "mov %0, %%rsi\n"      // buf = msg
        "mov %1, %%rdx\n"      // count = msg_len
        "syscall\n"
        :
        : "r"(msg), "r"(msg_len)
        : "rax", "rdi", "rsi", "rdx", "memory"
    );
    
    // Syscall: exit(status=0)
    // SYS_exit = 60
    __asm__ volatile(
        "mov $60, %%rax\n"     // syscall number: exit
        "mov $0, %%rdi\n"      // status = 0
        "syscall\n"
        :
        :
        : "rax", "rdi"
    );
    
    // Should never reach here
    while(1);
}
