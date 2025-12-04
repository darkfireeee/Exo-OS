// Minimal userspace test program for Exo-OS exec() testing
// Compiles to statically-linked ELF with no external dependencies

void _start() {
    // Write "Hello from execve!\n" to stdout (fd=1)
    const char msg[] = "Hello from execve!\n";
    unsigned long msg_len = 19;
    
    // Syscall: write(fd=1, buf=msg, count=19)
    // x86_64 syscall convention: rax=syscall_num, rdi=arg1, rsi=arg2, rdx=arg3
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
