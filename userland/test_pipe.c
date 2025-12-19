// Test pipe syscall - raw syscalls only
// No libc dependencies

void _start() {
    const char msg[] = "Testing pipe...\n";
    const char success_msg[] = "Pipe test OK!\n";
    
    // Write initial message
    __asm__ volatile(
        "mov $1, %%rax\n"
        "mov $1, %%rdi\n"
        "mov %0, %%rsi\n"
        "mov $16, %%rdx\n"
        "syscall\n"
        :
        : "r"(msg)
        : "rax", "rdi", "rsi", "rdx", "memory"
    );
    
    // Create pipe: pipe2(pipefd, 0)
    // SYS_pipe2 = 293
    int pipefd[2];
    __asm__ volatile(
        "mov $293, %%rax\n"    // syscall number: pipe2
        "mov %0, %%rdi\n"      // pipefd array
        "mov $0, %%rsi\n"      // flags = 0
        "syscall\n"
        :
        : "r"(pipefd)
        : "rax", "rdi", "rsi"
    );
    
    // Write to pipe
    const char pipe_msg[] = "Hello pipe!";
    __asm__ volatile(
        "mov $1, %%rax\n"      // SYS_write
        "mov %0, %%rdi\n"      // fd = pipefd[1]
        "mov %1, %%rsi\n"      // buf
        "mov $11, %%rdx\n"     // count
        "syscall\n"
        :
        : "r"((long)pipefd[1]), "r"(pipe_msg)
        : "rax", "rdi", "rsi", "rdx", "memory"
    );
    
    // Read from pipe
    char buf[32];
    __asm__ volatile(
        "mov $0, %%rax\n"      // SYS_read
        "mov %0, %%rdi\n"      // fd = pipefd[0]
        "mov %1, %%rsi\n"      // buf
        "mov $32, %%rdx\n"     // count
        "syscall\n"
        :
        : "r"((long)pipefd[0]), "r"(buf)
        : "rax", "rdi", "rsi", "rdx", "memory"
    );
    
    // Write success message
    __asm__ volatile(
        "mov $1, %%rax\n"
        "mov $1, %%rdi\n"
        "mov %0, %%rsi\n"
        "mov $14, %%rdx\n"
        "syscall\n"
        :
        : "r"(success_msg)
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
    
    while(1);
}
