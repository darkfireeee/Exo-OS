; ═══════════════════════════════════════════════════════════════════════════════
; Syscall Entry Point for x86_64
; ═══════════════════════════════════════════════════════════════════════════════
; SYSCALL instruction behavior:
;   - RCX = Return RIP (saved by CPU)
;   - R11 = Return RFLAGS (saved by CPU)
;   - RSP = unchanged (user stack!)
;   - RIP = IA32_LSTAR (this code)
;   - CS/SS loaded from IA32_STAR
;
; Syscall ABI (Linux compatible):
;   - RAX = syscall number
;   - RDI = arg1, RSI = arg2, RDX = arg3
;   - R10 = arg4, R8 = arg5, R9 = arg6
;   - RAX = return value (negative = -errno)
; ═══════════════════════════════════════════════════════════════════════════════

bits 64
default rel

section .data
; Per-CPU data offsets (GS base points to per-CPU area)
USER_RSP_OFFSET     equ 0       ; Saved user RSP
KERNEL_RSP_OFFSET   equ 8       ; Kernel stack pointer
CURRENT_TASK_OFFSET equ 16      ; Current task pointer

section .text

; ═══════════════════════════════════════════════════════════════════════════════
; syscall_entry - Main SYSCALL handler
; ═══════════════════════════════════════════════════════════════════════════════
global syscall_entry
syscall_entry:
    ; ─────────────────────────────────────────────────────────────────
    ; STEP 1: Switch to kernel GS and save user RSP
    ; ─────────────────────────────────────────────────────────────────
    swapgs                          ; Switch GS to kernel per-CPU data
    
    ; Save user RSP in per-CPU storage and load kernel RSP
    mov [gs:USER_RSP_OFFSET], rsp   ; Save user stack
    mov rsp, [gs:KERNEL_RSP_OFFSET] ; Load kernel stack
    
    ; ─────────────────────────────────────────────────────────────────
    ; STEP 2: Build syscall frame on kernel stack
    ; ─────────────────────────────────────────────────────────────────
    ; Save caller-saved registers and syscall return info
    push rcx                        ; User RIP (return address)
    push r11                        ; User RFLAGS
    push rbp
    push rbx
    push r12
    push r13
    push r14
    push r15
    
    ; Save syscall arguments for Rust handler
    push r9                         ; arg6
    push r8                         ; arg5
    push r10                        ; arg4 (NOT rcx, which has RIP)
    push rdx                        ; arg3
    push rsi                        ; arg2
    push rdi                        ; arg1
    push rax                        ; syscall number
    
    ; ─────────────────────────────────────────────────────────────────
    ; STEP 3: Call Rust syscall handler
    ; ─────────────────────────────────────────────────────────────────
    ; Args: rdi = syscall_num, rsi = arg1, rdx = arg2, 
    ;       rcx = arg3, r8 = arg4, r9 = arg5
    ; Note: arg6 is on stack
    mov rdi, rax                    ; syscall number -> rdi
    mov rax, [rsp + 8]              ; reload arg1
    mov rcx, [rsp + 24]             ; arg3 -> rcx (ABI difference)
    mov r8, [rsp + 32]              ; arg4 -> r8
    mov r9, [rsp + 40]              ; arg5 -> r9
    ; rsi, rdx already set correctly
    
    ; Re-enable interrupts in kernel
    sti
    
    ; Call Rust handler
    extern syscall_handler_rust
    call syscall_handler_rust
    
    ; Disable interrupts for return path
    cli
    
    ; RAX now contains return value
    
    ; ─────────────────────────────────────────────────────────────────
    ; STEP 4: Restore state and return to userspace
    ; ─────────────────────────────────────────────────────────────────
    ; Pop syscall args (we don't need them anymore)
    add rsp, 56                     ; 7 * 8 bytes (syscall_num + 6 args)
    
    ; Restore callee-saved registers
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    pop rbp
    pop r11                         ; User RFLAGS
    pop rcx                         ; User RIP
    
    ; Restore user RSP
    mov rsp, [gs:USER_RSP_OFFSET]
    
    ; Switch back to user GS
    swapgs
    
    ; Return to userspace
    ; SYSRETQ: RIP = RCX, RFLAGS = R11, CS/SS from IA32_STAR
    o64 sysret


; ═══════════════════════════════════════════════════════════════════════════════
; syscall_entry_simple - Simplified version for testing (no per-CPU data)
; ═══════════════════════════════════════════════════════════════════════════════
global syscall_entry_simple
syscall_entry_simple:
    ; Save user state
    push rcx                        ; User RIP
    push r11                        ; User RFLAGS
    push rbp
    push rbx
    push r12
    push r13
    push r14
    push r15
    push rdi
    push rsi
    push rdx
    push r10
    push r8
    push r9
    push rax                        ; syscall number at bottom
    
    ; Set up arguments for Rust handler
    ; syscall_handler_rust(syscall_num, arg1, arg2, arg3, arg4, arg5, arg6)
    mov rdi, rax                    ; syscall number
    mov rax, [rsp + 8*9]            ; arg1 (rdi was saved)
    xchg rdi, rax                   ; rdi = syscall_num, rax = arg1... no wait
    
    ; Let's do this more clearly:
    mov rdi, [rsp + 0]              ; rdi = syscall_num (rax)
    mov rsi, [rsp + 8*6]            ; rsi = arg1 (original rdi)
    mov rdx, [rsp + 8*5]            ; rdx = arg2 (original rsi)
    mov rcx, [rsp + 8*4]            ; rcx = arg3 (original rdx)
    mov r8, [rsp + 8*3]             ; r8 = arg4 (original r10)
    mov r9, [rsp + 8*2]             ; r9 = arg5 (original r8)
    ; arg6 is [rsp + 8*1] (original r9) - passed on stack for 7-arg call
    
    call syscall_handler_rust
    
    ; RAX has return value, save it
    mov [rsp + 0], rax              ; Store return value where syscall_num was
    
    ; Restore registers
    pop rax                         ; Return value
    pop r9
    pop r8
    pop r10
    pop rdx
    pop rsi
    pop rdi
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    pop rbp
    pop r11
    pop rcx
    
    ; Return to userspace
    o64 sysret


; ═══════════════════════════════════════════════════════════════════════════════
; set_kernel_stack - Set kernel stack for syscalls (called from Rust)
; ═══════════════════════════════════════════════════════════════════════════════
global set_kernel_stack
set_kernel_stack:
    ; RDI = kernel stack pointer
    mov [gs:KERNEL_RSP_OFFSET], rdi
    ret


; ═══════════════════════════════════════════════════════════════════════════════
; get_user_rsp - Get saved user RSP (called from Rust)
; ═══════════════════════════════════════════════════════════════════════════════
global get_user_rsp
get_user_rsp:
    mov rax, [gs:USER_RSP_OFFSET]
    ret
