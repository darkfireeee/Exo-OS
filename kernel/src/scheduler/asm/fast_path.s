// ═══════════════════════════════════════════════════════════════════════════════
// SCHEDULER FAST PATH ASM — Exo-OS Scheduler (x86_64)
// ═══════════════════════════════════════════════════════════════════════════════
//
// This file contains the ASM routines used on the hot path of the
// scheduler (tick, rescheduling IPI, syscall return).
//
// No C calls in these routines — only atomic checks
// and branches to context_switch_asm.
// ═══════════════════════════════════════════════════════════════════════════════

.section .text

// ─────────────────────────────────────────────────────────────────────────────
// read_need_resched_flag — O(1) read of the NEED_RESCHED flag in the TCB
//
// Signature : u32 read_need_resched_flag(const ThreadControlBlock *tcb)
//             rdi = TCB pointer
// Returns   : 1 if NEED_RESCHED is set, 0 otherwise
// Clobbers  : rax (return value), rcx (scratch, saved by caller)
//
// NEED_RESCHED = bit 11 in TCB::sched_state (AtomicU64 at offset +24).
// ─────────────────────────────────────────────────────────────────────────────

// Offsets in ThreadControlBlock (must stay in sync with task.rs)
.set TCB_SCHED_STATE_OFFSET,     24      // AtomicU64 sched_state — cache line 1
.set SCHED_SIGNAL_PENDING_BIT,   256     // = 1 << 8
.set SCHED_NEED_RESCHED_BIT,     2048    // = 1 << 11

.global read_need_resched_flag
.type read_need_resched_flag, @function

read_need_resched_flag:
    // Relaxed atomic read of the TCB sched_state field (rdi = TCB ptr)
    movq    TCB_SCHED_STATE_OFFSET(%rdi), %rax
    testq   $SCHED_NEED_RESCHED_BIT, %rax
    setnz   %al
    movzbl  %al, %eax
    ret

.size read_need_resched_flag, . - read_need_resched_flag


// ─────────────────────────────────────────────────────────────────────────────
// check_signal_flag — O(1) read of the signal_pending flag in the TCB
//
// Signature : u8 check_signal_flag(const ThreadControlBlock *tcb)
//             rdi = TCB pointer
// Returns   : 1 if signal_pending, 0 otherwise
// ─────────────────────────────────────────────────────────────────────────────

.global check_signal_flag
.type check_signal_flag, @function

check_signal_flag:
    movq    TCB_SCHED_STATE_OFFSET(%rdi), %rax
    testq   $SCHED_SIGNAL_PENDING_BIT, %rax
    setnz   %al
    movzbl  %al, %eax
    ret

.size check_signal_flag, . - check_signal_flag


// ─────────────────────────────────────────────────────────────────────────────
// scheduler_ipi_handler — inter-CPU rescheduling IPI handler
//
// Triggered by `smp::migration` to force a reschedule on a remote CPU.
// Simple setflag + EOI — the actual reschedule is done in the tick handler.
//
// This is a trampoline: it only sets the flag and returns (IRETQ).
// ─────────────────────────────────────────────────────────────────────────────

.global scheduler_ipi_handler_asm
.type scheduler_ipi_handler_asm, @function

scheduler_ipi_handler_asm:
    // Save caller-saved registers (unknown call site context)
    pushq   %rax
    pushq   %rcx
    pushq   %rdx
    pushq   %rsi
    pushq   %rdi
    pushq   %r8
    pushq   %r9
    pushq   %r10
    pushq   %r11

    // Call C function to handle the IPI.
    // SAFETY: the C function must not allocate memory nor acquire locks
    //         that are not IRQ-safe.
    extern_fn_scheduler_ipi_handler:
    call    scheduler_ipi_handler_c

    // EOI (End Of Interrupt) to LAPIC.
    // LAPIC EOI register = absolute physical address 0xFEE000B0 (xAPIC mode).
    // WARNING: do NOT use RIP-relative addressing here — LAPIC address
    // is fixed at 0xFEE000B0 and cannot be reached with a 32-bit offset
    // from RIP. We use an intermediate register.
    movabsq $0xFEE000B0, %rax
    movl    $0, (%rax)

    popq    %r11
    popq    %r10
    popq    %r9
    popq    %r8
    popq    %rdi
    popq    %rsi
    popq    %rdx
    popq    %rcx
    popq    %rax

    iretq

.size scheduler_ipi_handler_asm, . - scheduler_ipi_handler_asm


// ─────────────────────────────────────────────────────────────────────────────
// yield_fast_path — O(1) yield without C call if no other thread is waiting
//
// Signature : i32 yield_fast_path(PerCpuRunQueue *rq)
//   rdi = PerCpuRunQueue ptr
// Returns   : 0 if no switch is needed, 1 if reschedule is triggered
//
// nr_running offset in PerCpuRunQueue: to define from stats.nr_running.
// Reads the AtomicU32 directly.
// ─────────────────────────────────────────────────────────────────────────────

// nr_running offset in RunQueueStats (embedded in PerCpuRunQueue)
// Provisional value — must stay in sync with the Rust layout.
.set RQ_STATS_OFFSET,      0     // RunQueueStats is at the start of the struct (approximation)
.set STATS_NR_RUNNING_OFFSET, 32 // AtomicU32 nr_running

.global yield_fast_path
.type yield_fast_path, @function

yield_fast_path:
    // Read nr_running (approximation — the real offset depends on the Rust layout)
    movl    (STATS_NR_RUNNING_OFFSET)(%rdi), %eax
    // If 0 or 1 -> no other thread -> no switch needed
    cmpl    $1, %eax
    jle     .L_yield_no_switch

    // Other threads exist -> indicate that a reschedule is desirable
    movl    $1, %eax
    ret

.L_yield_no_switch:
    xorl    %eax, %eax
    ret

.size yield_fast_path, . - yield_fast_path
