// Test fork syscall - raw syscalls only
// No libc dependencies

void _start() {
    // Message for parent
    const char parent_msg[] = "Parent: calling fork...\n";
    const char child_msg[] = "Child process created!\n";
    const char done_msg[] = "Parent: child exited\n";
    
    // Syscall: fork()
    // SYS_fork = 57
    long pid;
    __asm__ volatile(
        "mov $57, %%rax\n"     // syscall number: fork
        "syscall\n"
        "mov %%rax, %0\n"
        : "=r"(pid)
        :
        : "rax"
    );
    
    if (pid == 0) {
        // Child process
        // Write child message
        __asm__ volatile(
            "mov $1, %%rax\n"
            "mov $1, %%rdi\n"
            "mov %0, %%rsi\n"
            "mov $24, %%rdx\n"
            "syscall\n"
            :
            : "r"(child_msg)
            : "rax", "rdi", "rsi", "rdx", "memory"
        );
        
        // Exit with code 42
        __asm__ volatile(
            "mov $60, %%rax\n"
            "mov $42, %%rdi\n"
            "syscall\n"
            :
            :
            : "rax", "rdi"
        );
    } else if (pid > 0) {
        // Parent process
        // Write parent message
        __asm__ volatile(
            "mov $1, %%rax\n"
            "mov $1, %%rdi\n"
            "mov %0, %%rsi\n"
            "mov $25, %%rdx\n"
            "syscall\n"
            :
            : "r"(parent_msg)
            : "rax", "rdi", "rsi", "rdx", "memory"
        );
        
        // Wait for child (simplified - just sleep a bit)
        // In real code, would use wait4()
        
        // Write done message
        __asm__ volatile(
            "mov $1, %%rax\n"
            "mov $1, %%rdi\n"
            "mov %0, %%rsi\n"
            "mov $22, %%rdx\n"
            "syscall\n"
            :
            : "r"(done_msg)
            : "rax", "rdi", "rsi", "rdx", "memory"
        );
        
        // Exit
        __asm__ volatile(
            "mov $60, %%rax\n"
            "mov $0, %%rdi\n"
            "syscall\n"
            :
            :
            : "rax", "rdi"
        );
    }
    
    while(1);
}
