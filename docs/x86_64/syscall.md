# 📞 System Calls (SYSCALL/SYSRET)

## Vue d'ensemble

Exo-OS utilise `SYSCALL/SYSRET` pour les appels système rapides (~50 cycles d'overhead).

## Configuration

```rust
pub fn init_syscall() {
    unsafe {
        // Enable SYSCALL/SYSRET in EFER
        let efer = rdmsr(IA32_EFER);
        wrmsr(IA32_EFER, efer | 1); // SCE bit
        
        // STAR: Segment selectors
        // Bits 32-47: Kernel CS/SS (CS = STAR[32:47], SS = STAR[32:47] + 8)
        // Bits 48-63: User CS/SS (CS = STAR[48:63] + 16, SS = STAR[48:63] + 8)
        let star = ((0x08u64) << 32) | ((0x18u64) << 48);
        wrmsr(IA32_STAR, star);
        
        // LSTAR: Syscall entry point
        wrmsr(IA32_LSTAR, syscall_entry as u64);
        
        // FMASK: Flags to clear on syscall
        wrmsr(IA32_FMASK, 0x200); // Clear IF (disable interrupts)
    }
}
```

## Entry Point Assembly

```asm
syscall_entry:
    ; Swap to kernel GS
    swapgs
    
    ; Save user stack
    mov gs:[USER_RSP], rsp
    
    ; Load kernel stack
    mov rsp, gs:[KERNEL_RSP]
    
    ; Save registers
    push rcx        ; User RIP
    push r11        ; User RFLAGS
    push rbp
    push rbx
    push r12
    push r13
    push r14
    push r15
    
    ; Call Rust handler
    ; rax = syscall number
    ; rdi, rsi, rdx, r10, r8, r9 = arguments
    mov rcx, r10    ; Argument 4 (Linux ABI uses r10)
    call syscall_dispatch
    
    ; Restore registers
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    pop rbp
    pop r11         ; User RFLAGS
    pop rcx         ; User RIP
    
    ; Restore user stack
    mov rsp, gs:[USER_RSP]
    
    ; Swap back to user GS
    swapgs
    
    ; Return to userspace
    sysretq
```

## Dispatch Rust

```rust
#[no_mangle]
pub extern "C" fn syscall_dispatch(
    num: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
    arg5: u64,
    arg6: u64,
) -> u64 {
    match num {
        // Process
        SYS_EXIT => sys_exit(arg1 as i32),
        SYS_FORK => sys_fork(),
        SYS_EXEC => sys_exec(arg1 as *const u8, arg2, arg3),
        
        // IPC
        SYS_IPC_SEND => sys_ipc_send(arg1, arg2 as *const u8, arg3),
        SYS_IPC_RECV => sys_ipc_recv(arg1, arg2 as *mut u8, arg3),
        SYS_IPC_CREATE => sys_ipc_create(arg1, arg2),
        
        // Memory
        SYS_MMAP => sys_mmap(arg1, arg2, arg3, arg4, arg5 as i32, arg6),
        SYS_MUNMAP => sys_munmap(arg1, arg2),
        
        // Files
        SYS_OPEN => sys_open(arg1 as *const u8, arg2 as i32, arg3 as u32),
        SYS_CLOSE => sys_close(arg1 as i32),
        SYS_READ => sys_read(arg1 as i32, arg2 as *mut u8, arg3),
        SYS_WRITE => sys_write(arg1 as i32, arg2 as *const u8, arg3),
        
        _ => (-1i64) as u64, // ENOSYS
    }
}
```

## Numéros de Syscalls

```rust
// Process management
pub const SYS_EXIT: u64 = 0;
pub const SYS_FORK: u64 = 1;
pub const SYS_EXEC: u64 = 2;
pub const SYS_WAIT: u64 = 3;
pub const SYS_GETPID: u64 = 4;

// IPC (Exo-OS specific)
pub const SYS_IPC_CREATE: u64 = 100;
pub const SYS_IPC_SEND: u64 = 101;
pub const SYS_IPC_RECV: u64 = 102;
pub const SYS_IPC_CLOSE: u64 = 103;

// Memory
pub const SYS_MMAP: u64 = 200;
pub const SYS_MUNMAP: u64 = 201;
pub const SYS_MPROTECT: u64 = 202;

// Files
pub const SYS_OPEN: u64 = 300;
pub const SYS_CLOSE: u64 = 301;
pub const SYS_READ: u64 = 302;
pub const SYS_WRITE: u64 = 303;
```

## Performance

| Aspect | Exo-OS | Linux |
|--------|--------|-------|
| Syscall overhead | ~50 cycles | ~100 cycles |
| IPC syscall | ~100 cycles | ~1200 cycles |
| Context save | Minimal | Full |
