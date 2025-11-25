# Windowed Context Switch - Ultra-fast 16-byte context
#
# Context: RSP (8 bytes) + RIP (8 bytes) = 16 bytes total
# Performance: 2 MOV + 1 JMP = ~350 cycles target
#
# Calling convention (x86_64 SysV ABI):
# rdi = &old_rsp (pointer to save old RSP)
# rsi = new_rsp (new stack pointer to restore)

.intel_syntax noprefix
.text

.global windowed_context_switch
windowed_context_switch:
    # Save current RSP to *old_rsp
    mov QWORD PTR [rdi], rsp
    
    # Load new RSP from new_rsp
    mov rsp, rsi
    
    # Return to new thread (RIP is on top of new stack)
    ret

# Fast path for first-time thread entry
.global windowed_thread_entry
windowed_thread_entry:
    # New threads have entry point on stack
    # Just pop and jump
    pop rax
    jmp rax

# Save callee-saved registers (for full context switch)
.global windowed_save_callee
windowed_save_callee:
    push rbx
    push rbp
    push r12
    push r13
    push r14
    push r15
    ret

# Restore callee-saved registers
.global windowed_restore_callee
windowed_restore_callee:
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbp
    pop rbx
    ret

# Minimal switch (RSP+RIP only, no callee-saved)
.global windowed_minimal_switch
windowed_minimal_switch:
    # rdi = &old_context (16 bytes: RSP + RIP)
    # rsi = &new_context (16 bytes: RSP + RIP)
    
    # Save current context
    mov QWORD PTR [rdi], rsp            # Save RSP
    lea rax, QWORD PTR [rip + .Lreturn] # Get return address
    mov QWORD PTR [rdi + 8], rax        # Save RIP
    
    # Load new context
    mov rsp, QWORD PTR [rsi]            # Restore RSP
    jmp QWORD PTR [rsi + 8]             # Jump to RIP
    
.Lreturn:
    ret
