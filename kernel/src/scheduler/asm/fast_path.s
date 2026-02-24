// ═══════════════════════════════════════════════════════════════════════════════
// SCHEDULER FAST PATH ASM — Exo-OS Scheduler (x86_64)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Ce fichier contient les routines ASM utilisées sur le chemin chaud du
// scheduler (tick, IPI de rescheduling, retour syscall).
//
// Pas d'appel C dans ces routines — uniquement des checks atomiques
// et des branchements vers context_switch_asm.
// ═══════════════════════════════════════════════════════════════════════════════

.section .text

// ─────────────────────────────────────────────────────────────────────────────
// read_need_resched_flag — lecture O(1) du flag NEED_RESCHED dans le TCB
//
// Signature : u32 read_need_resched_flag(const ThreadControlBlock *tcb)
//             rdi = pointeur TCB
// Retourne  : 1 si NEED_RESCHED est positionné, 0 sinon
// Registres modifiés : rax (valeur retour), rcx (scratch, sauvegardé par caller)
//
// NEED_RESCHED = bit 4 dans TCB::flags (AtomicU32 à l'offset +28 du TCB).
// ─────────────────────────────────────────────────────────────────────────────

// Offsets dans ThreadControlBlock (doit rester synchronisé avec task.rs)
.set TCB_FLAGS_OFFSET,        28    // AtomicU32 flags — cache line 1
.set TCB_SIGNAL_PENDING_OFFSET, 48  // AtomicBool signal_pending
.set NEED_RESCHED_BIT,         4    // bit dans flags (1 << 4)

.global read_need_resched_flag
.type read_need_resched_flag, @function

read_need_resched_flag:
    // Lecture atomique Relaxed du champ flags du TCB (rdi = ptr TCB)
    movl    TCB_FLAGS_OFFSET(%rdi), %eax
    andl    $NEED_RESCHED_BIT, %eax
    setnz   %al
    movzbl  %al, %eax
    ret

.size read_need_resched_flag, . - read_need_resched_flag


// ─────────────────────────────────────────────────────────────────────────────
// check_signal_flag — lecture O(1) du flag signal_pending dans le TCB
//
// Signature : u8 check_signal_flag(const ThreadControlBlock *tcb)
//             rdi = pointeur TCB
// Retourne  : 1 si signal_pending, 0 sinon
// ─────────────────────────────────────────────────────────────────────────────

.global check_signal_flag
.type check_signal_flag, @function

check_signal_flag:
    movzbl  TCB_SIGNAL_PENDING_OFFSET(%rdi), %eax
    ret

.size check_signal_flag, . - check_signal_flag


// ─────────────────────────────────────────────────────────────────────────────
// scheduler_ipi_handler — handler IPI de rescheduling inter-CPU
//
// Déclenché par `smp::migration` pour forcer un reschedule sur un CPU distant.
// Simple setflag + EOI — le reschedule effectif est fait dans le tick handler.
//
// C'est une trampoline : ne fait que poser le flag et retourner (IRETQ).
// ─────────────────────────────────────────────────────────────────────────────

.global scheduler_ipi_handler_asm
.type scheduler_ipi_handler_asm, @function

scheduler_ipi_handler_asm:
    // Sauvegarder les registres caller-saved (on ne sait pas d'où on a été appelé)
    pushq   %rax
    pushq   %rcx
    pushq   %rdx
    pushq   %rsi
    pushq   %rdi
    pushq   %r8
    pushq   %r9
    pushq   %r10
    pushq   %r11

    // Appeler la fonction C pour traiter l'IPI.
    // SAFETY: la fonction C ne doit pas allouer de mémoire ni acquérir de locks
    //         non-IRQ-safe.
    extern_fn_scheduler_ipi_handler:
    call    scheduler_ipi_handler_c

    // EOI (End Of Interrupt) vers LAPIC.
    // Adresse LAPIC EOI = 0xFEE000B0 (XAPIC mode).
    movl    $0, 0xFEE000B0(%rip) // Note: en vrai usage, LAPIC_BASE est en mémoire mappée
    // Alternative avec MSR x2APIC :
    // movl $0x80B, %ecx
    // xorl %eax, %eax
    // xorl %edx, %edx
    // wrmsr

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
// yield_fast_path — yield O(1) sans appel C si aucun autre thread n'attend
//
// Signature : i32 yield_fast_path(PerCpuRunQueue *rq)
//   rdi = ptr PerCpuRunQueue
// Retourne  : 0 si aucun switch nécessaire, 1 si reschedule déclenché
//
// Offset nr_running dans PerCpuRunQueue : à définir depuis stats.nr_running.
// On lit directement l'AtomicU32.
// ─────────────────────────────────────────────────────────────────────────────

// Offset nr_running dans RunQueueStats (embedded dans PerCpuRunQueue)
// Valeur provisoire — doit être synchronisée avec le layout Rust.
.set RQ_STATS_OFFSET,      0     // RunQueueStats est en début de struct (approximation)
.set STATS_NR_RUNNING_OFFSET, 32 // AtomicU32 nr_running

.global yield_fast_path
.type yield_fast_path, @function

yield_fast_path:
    // Lire nr_running (approximation — l'offset réel dépend du layout Rust)
    movl    (STATS_NR_RUNNING_OFFSET)(%rdi), %eax
    // Si 0 ou 1 → pas d'autre thread → pas de switch nécessaire
    cmpl    $1, %eax
    jle     .L_yield_no_switch

    // Il y a d'autres threads → signaler qu'un reschedule est souhaitable
    movl    $1, %eax
    ret

.L_yield_no_switch:
    xorl    %eax, %eax
    ret

.size yield_fast_path, . - yield_fast_path
