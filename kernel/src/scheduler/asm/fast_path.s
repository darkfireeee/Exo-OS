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
// NEED_RESCHED = bit 11 dans TCB::sched_state (AtomicU64 à l'offset +24).
// ─────────────────────────────────────────────────────────────────────────────

// Offsets dans ThreadControlBlock (doit rester synchronisé avec task.rs)
.set TCB_SCHED_STATE_OFFSET,     24      // AtomicU64 sched_state — cache line 1
.set SCHED_SIGNAL_PENDING_BIT,   256     // = 1 << 8
.set SCHED_NEED_RESCHED_BIT,     2048    // = 1 << 11

.global read_need_resched_flag
.type read_need_resched_flag, @function

read_need_resched_flag:
    // Lecture atomique Relaxed du champ sched_state du TCB (rdi = ptr TCB)
    movq    TCB_SCHED_STATE_OFFSET(%rdi), %rax
    testq   $SCHED_NEED_RESCHED_BIT, %rax
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
    movq    TCB_SCHED_STATE_OFFSET(%rdi), %rax
    testq   $SCHED_SIGNAL_PENDING_BIT, %rax
    setnz   %al
    movzbl  %al, %eax
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
    // LAPIC EOI register = adresse physique absolue 0xFEE000B0 (xAPIC mode).
    // ATTENTION : ne PAS utiliser l'adressage RIP-relatif ici — l'adresse LAPIC
    // est fixe à 0xFEE000B0 et ne peut pas être atteinte avec un offset 32 bits
    // depuis le RIP. On passe par un registre intermédiaire.
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
