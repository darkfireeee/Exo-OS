// Test binary for exec() VFS loading - Jour 2
// This is a minimal userland binary loaded via load_elf_binary()

// Syscall numbers (Linux x86_64)
#define SYS_write 1
#define SYS_exit 60

// Syscall wrapper
static inline long syscall3(long n, long a1, long a2, long a3) {
    long ret;
    asm volatile(
        "syscall"
        : "=a"(ret)
        : "a"(n), "D"(a1), "S"(a2), "d"(a3)
        : "rcx", "r11", "memory"
    );
    return ret;
}

// Entry point
void _start(void) {
    const char *msg = "SUCCESS: Loaded from VFS!\n";
    long len = 26;
    
    // sys_write(1, msg, len)
    syscall3(SYS_write, 1, (long)msg, len);
    
    // sys_exit(0)
    syscall3(SYS_exit, 0, 0, 0);
    
    // Never reached
    while(1);
}
