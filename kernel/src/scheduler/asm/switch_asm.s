// ═══════════════════════════════════════════════════════════════════════════════
// CONTEXT SWITCH ASM — Exo-OS Scheduler (x86_64)
// ═══════════════════════════════════════════════════════════════════════════════
//
// RULE SWITCH-ASM (V7-C-02 + CORR-18):
//   • Save ONLY the 6 System V ABI callee-saved registers:
//     rbx, rbp, r12, r13, r14, r15   (6 × 8 = 48B + implicit rip = 56B)
//   • DO NOT touch MXCSR nor x87 FCW:
//     The kernel is compiled with -mmx,-sse,-sse2,+soft-float -> no compiler-generated
//     SSE instructions -> MXCSR cannot be corrupted by the
//     kernel. The full FPU state (MXCSR, FCW, x87/SSE/AVX registers) is managed
//     exclusively by scheduler/fpu/save_restore.rs via XSAVE/XRSTOR.
//   • DO NOT touch FS/GS base: handled by switch.rs via rdmsr/wrmsr (CORR-11).
//   • CR3 switched HERE, BEFORE restoring registers (atomic KPTI)
//
// Note: The TCB contains 15 GPRs for IRQ preemption (the IRQ handler
//       pushes all GPRs). switch_asm.s is optimized for cooperative yield
//       only (6 callee-saved GPRs). These two paths are distinct
//       and compatible — see CORR-18 for the full explanation.
//
// Signature (System V ABI):
//   context_switch_asm(
//     old_kernel_rsp: *mut u64,   // rdi
//     new_kernel_rsp: u64,        // rsi
//     new_cr3:        u64,        // rdx  (0 = no CR3 switch)
//   )
//
// ═══════════════════════════════════════════════════════════════════════════════

.section .text
.global context_switch_asm
.type context_switch_asm, @function

context_switch_asm:
    // ── Save the 6 callee-saved registers of the OUTGOING thread ─────────────
    // (V7-C-02: WITHOUT MXCSR nor x87 FCW — managed by XSAVE/XRSTOR in fpu/)
    pushq   %r15
    pushq   %r14
    pushq   %r13
    pushq   %r12
    pushq   %rbp
    pushq   %rbx
    // Total: 6 × 8 = 48B on stack (+ implicit rip = 56B)

    // Save OUTGOING thread RSP in its TCB (kstack_ptr field).
    // rdi = &TCB::kstack_ptr of the outgoing thread
    movq    %rsp, (%rdi)

    // ── Switch CR3 if necessary (KPTI) ───────────────────────────────────────
    // rdx = new_cr3 (0 = same address space, no TLB flush)
    testq   %rdx, %rdx
    jz      .L_skip_cr3

    // Atomic CR3 switch — invalidates user TLB automatically.
    // CR3 is switched BEFORE restoring registers (KPTI invariant).
    movq    %rdx, %cr3

.L_skip_cr3:
    // ── Load INCOMING thread RSP from its TCB (kstack_ptr field) ─────────────
    // rsi = TCB::kstack_ptr of incoming thread
    movq    %rsi, %rsp

    // ── Restore the 6 callee-saved registers of the INCOMING thread ───────────
    popq    %rbx
    popq    %rbp
    popq    %r12
    popq    %r13
    popq    %r14
    popq    %r15

    // Return — resume in the context of the new thread.
    // ret consumes the return address pushed during the previous
    // context_switch_asm() call for this thread (when it was outgoing).
    ret

.size context_switch_asm, . - context_switch_asm


// ═══════════════════════════════════════════════════════════════════════════════
// SWITCH TO A NEW THREAD — First start (never switched before)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Used when creating a thread. The new thread's kernel stack
// is prepared with the address of `thread_entry_trampoline` as the return address.
//
// Signature:
//   switch_to_new_thread(
//     old_kernel_rsp: *mut u64,   // rdi — NULL if bootstrap
//     new_kernel_rsp: u64,        // rsi
//     new_cr3:        u64,        // rdx
//   )
//
// ═══════════════════════════════════════════════════════════════════════════════

.section .text
.global switch_to_new_thread
.type switch_to_new_thread, @function

switch_to_new_thread:
    // Save prev only if old_kernel_rsp != NULL
    testq   %rdi, %rdi
    jz      .L_no_save

    // FIX-SWITCH-ASM-01: save ONLY the 6 callee-saved registers,
    // WITHOUT the 16 bytes MXCSR+FCW (V7-C-02: managed by XSAVE/XRSTOR).
    // Identical layout to context_switch_asm -> compatible restoration.
    pushq   %r15
    pushq   %r14
    pushq   %r13
    pushq   %r12
    pushq   %rbp
    pushq   %rbx
    // Total: 6 × 8 = 48B on stack (+ implicit rip = 56B)
    movq    %rsp, (%rdi)

.L_no_save:
    // Switch CR3 if necessary
    testq   %rdx, %rdx
    jz      .L_new_no_cr3
    movq    %rdx, %cr3

.L_new_no_cr3:
    // Load the new stack pointer
    movq    %rsi, %rsp

    // For a brand new thread, the "stack" was prepared by the Rust thread
    // creation code with the following layout (WITHOUT the MXCSR+FCW area):
    //   [kernel_rsp+ 0] = initial rbx (0)
    //   [kernel_rsp+ 8] = initial rbp (0)
    //   [kernel_rsp+16] = initial r12 (0)
    //   [kernel_rsp+24] = initial r13 (0)
    //   [kernel_rsp+32] = initial r14 (0)
    //   [kernel_rsp+40] = initial r15 (0)
    //   [kernel_rsp+48] = return address -> thread_entry_trampoline
    //
    // Pop the 6 registers (48 bytes), then ret jumps to thread_entry_trampoline.
    popq    %rbx
    popq    %rbp
    popq    %r12
    popq    %r13
    popq    %r14
    popq    %r15
    // The 16 bytes MXCSR+FCW are not on the initial stack -> skip
    ret

.size switch_to_new_thread, . - switch_to_new_thread


// ═══════════════════════════════════════════════════════════════════════════════
// KTHREAD TRAMPOLINE — First start of a kthread (Exo-OS)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Called during the FIRST context_switch to a newly created kthread.
//
// Context: context_switch_asm restored the frame prepared by create_kthread():
//   r12 = entry_fn   (fn(usize)->!)   <- r12 slot of stack frame
//   r13 = arg        (usize)          <- r13 slot of stack frame
//
// SystemV AMD64 convention:
//   First integer argument -> rdi
//
// This trampoline moves arg into rdi then jumps to entry_fn.
// entry_fn is declared -> ! so it NEVER returns.
// ═══════════════════════════════════════════════════════════════════════════════

.section .text
.global kthread_trampoline
.type kthread_trampoline, @function

kthread_trampoline:
    movq    %r13, %rdi      // arg -> rdi (1st parameter SystemV AMD64)
    sti                     // first kthread context: IF must be active
    jmpq    *%r12           // jump to entry_fn(arg) — never returns

.size kthread_trampoline, . - kthread_trampoline


// ═══════════════════════════════════════════════════════════════════════════════
// FORK CHILD TRAMPOLINE — First start of a child process (fork)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Called by switch_to_new_thread during the FIRST scheduling of a process
// created via do_fork(). The child's kernel stack was prepared by do_fork():
//
//   [kernel_rsp + 0..48)   = 6 callee-saved registers = 0 (switch_to_new_thread format)
//   [kernel_rsp + 48]      = fork_child_trampoline  <- switch_to_new_thread return address
//   [kernel_rsp + 56..104) = userspace caller-saved regs (rdi,rsi,rdx,r10,r8,r9)
//   [kernel_rsp + 104]     = child_rip  <- RSP points here after the 6 pops below
//   [kernel_rsp + 112]     = GDT_USER_CS64 (CS ring3)
//   [kernel_rsp + 120]     = 0x0202    (RFLAGS : IF=1)
//   [kernel_rsp + 128]     = child_rsp (userspace RSP)
//   [kernel_rsp + 136]     = GDT_USER_DS   (SS ring3)
//
// Invariant: GS = kernel GS (scheduler has NOT executed SWAPGS).
//            We must execute SWAPGS before IRETQ to restore userspace GS.
//
// RULE PROC-08: Parent TLB flushed in do_fork() — no flush needed here.
// ═══════════════════════════════════════════════════════════════════════════════

.section .text
.global fork_child_trampoline
.type fork_child_trampoline, @function

fork_child_trampoline:
    popq    %rdi            // restore visible registers after syscall
    popq    %rsi
    popq    %rdx
    popq    %r10
    popq    %r8
    popq    %r9
    xor     %eax, %eax      // rax = 0: child returns 0 from fork()
    clts                    // allow FPU/SIMD upon first Ring3 return
    movq    %gs:0x48, %rax  // KPTI user CR3 published by switch.rs (0 if inactive)
    testq   %rax, %rax
    jz      .L_fork_child_no_kpti
    movq    %rax, %cr3
.L_fork_child_no_kpti:
    xor     %eax, %eax      // preserve fork() semantics: child return = 0
    swapgs                  // restore userspace GS (kernel had kernel GS)
    iretq                   // pop RIP, CS, RFLAGS, RSP, SS -> return to Ring3

.size fork_child_trampoline, . - fork_child_trampoline


// ═══════════════════════════════════════════════════════════════════════════════
// USER ENTRY TRAMPOLINE — First start of PID 1 / initial exec
// ═══════════════════════════════════════════════════════════════════════════════
//
// Identical layout to fork child frame:
//   [kernel_rsp + 48] = user_entry_trampoline
//   [kernel_rsp + 56] = userspace RIP
//   [kernel_rsp + 64] = ring3 CS
//   [kernel_rsp + 72] = RFLAGS
//   [kernel_rsp + 80] = userspace RSP
//   [kernel_rsp + 88] = ring3 SS
//
// Unlike fork(), this path must not inherit parent registers.
// r12 conveys the optional ABI argument provided by ElfLoadResult::entry_arg0.

.section .text
.global user_entry_trampoline
.type user_entry_trampoline, @function

user_entry_trampoline:
    xor     %eax, %eax      // rax = 0
    mov     %r12, %rdi      // rdi = handoff/boot_info/null for _start()
    xor     %esi, %esi      // rsi = 0
    xor     %edx, %edx      // rdx = 0
    clts                    // Ring1 bootstrap can use core::ptr SSE
    movq    %gs:0x48, %rax  // KPTI user CR3 published by switch.rs (0 if inactive)
    testq   %rax, %rax
    jz      .L_user_entry_no_kpti
    movq    %rax, %cr3
.L_user_entry_no_kpti:
    swapgs
    xor     %eax, %eax
    iretq

.size user_entry_trampoline, . - user_entry_trampoline
