// kernel/src/scheduler/core/switch.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// CONTEXT SWITCH — Dispatch vers switch_asm.s (Exo-OS Scheduler · Couche 1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLES (DOC1 + DOC3) :
//   RÈGLE SWITCH-01 : check_signal_pending() LIT uniquement — jamais de livraison
//   RÈGLE SWITCH-02 : Lazy FPU AVANT le switch, mark APRÈS
//   RÈGLE SIGNAL-01 : scheduler/ NE connaît PAS process::signal::*
//                     Il lit seulement le flag AtomicBool signal_pending du TCB
//   RÈGLE SWITCH-ASM : switch_asm.s sauvegarde rbx,rbp,r12-r15,rsp + MXCSR + x87 FCW
//                      CR3 switché dans switch_asm AVANT restauration des registres (KPTI)
//   ZONE NO-ALLOC : aucune allocation dans ce chemin chaud
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::Ordering;
use super::task::{ThreadControlBlock, TaskState};
use crate::scheduler::fpu;

// ─────────────────────────────────────────────────────────────────────────────
// FFI vers l'ASM de context switch
// ─────────────────────────────────────────────────────────────────────────────

extern "C" {
    /// Context switch ASM complet.
    ///
    /// Sauvegarde les registres callee-saved (rbx, rbp, r12-r15) + MXCSR + x87 FCW
    /// du thread `old`, puis switche CR3 si nécessaire (KPTI), puis restaure
    /// le contexte du thread `new`.
    ///
    /// # Arguments (System V ABI)
    /// - `old_kernel_rsp` : `*mut u64` pointant vers `TCB::kernel_rsp` du thread sortant
    /// - `new_kernel_rsp` : valeur du `TCB::kernel_rsp` du thread entrant
    /// - `new_cr3`        : registre CR3 du thread entrant (0 = pas de switch CR3)
    fn context_switch_asm(
        old_kernel_rsp: *mut u64,
        new_kernel_rsp:  u64,
        new_cr3:         u64,
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Signal pending check — HOT PATH, ≤ 5 cycles
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie si un signal est en attente sur ce thread.
///
/// RÈGLE SWITCH-01 :
///   - LIT uniquement le flag `signal_pending` du TCB.
///   - NE livre PAS les signaux — la livraison s'effectue depuis arch/syscall.rs
///     ou arch/exceptions.rs au retour vers userspace.
///   - NE connaît PAS `process::signal::*`.
///
/// En hot path scheduler, Ordering::Relaxed est correct : la cohérence
/// de vue sera établie par l'Acquire au retour userspace dans arch/.
#[inline(always)]
pub fn check_signal_pending(tcb: &ThreadControlBlock) -> bool {
    tcb.signal_pending.load(Ordering::Relaxed)
}

// ─────────────────────────────────────────────────────────────────────────────
// context_switch() — point d'entrée Rust du switch
// ─────────────────────────────────────────────────────────────────────────────

/// Effectue le context switch de `prev` vers `next`.
///
/// # Séquence
/// 1. Lazy FPU : si `prev` a utilisé la FPU → la sauvegarder (XSAVE/FXSAVE).
/// 2. Marquer `prev` comme non-Running → Runnable (sauf si en train de mourir/dormir).
/// 3. Appeler `context_switch_asm(prev_rsp_ptr, next_rsp, next_cr3)`.
///    La fonction ASM sauvegarde/restaure les callee-saved + MXCSR + x87 FCW.
///    CR3 est switché atomiquement si `prev.cr3 != next.cr3`.
/// 4. De retour côté `next` (après restauration par ASM) :
///    marquer `next` comme Running.
///    Invalider le flag FPU_LOADED pour `next` (lazy restore).
///
/// # Sécurité
/// - Appelé avec préemption désactivée (IrqGuard ou PreemptGuard).
/// - `prev` et `next` DOIVENT être des pointeurs valides.
///
/// # RÈGLE ABSOLUE
/// Cette fonction NE doit JAMAIS appeler `process::signal::*`.
/// Elle ne fait que lire `signal_pending` via `check_signal_pending()`.
pub unsafe fn context_switch(
    prev: &mut ThreadControlBlock,
    next: &mut ThreadControlBlock,
) {
    // ── Étape 1 : Lazy FPU save ──────────────────────────────────────────────
    // RÈGLE SWITCH-02 : sauvegarder FPU du thread sortant si elle était chargée.
    if prev.fpu_loaded() {
        fpu::save_restore::xsave_current(prev);
        // On ne mark pas FPU_LOADED = false ici — c'est fait dans step 4.
        // (Si un IRQ arrive entre step 1 et step 3, la FPU est déjà sauvée.)
    }

    // ── Étape 2 : Transition d'état de prev ──────────────────────────────────
    // Si le thread sortant était Running → il redevient Runnable (sera ré-enfilé).
    // Si il était dans un état bloquant (Sleeping, Uninterruptible) → on ne change pas.
    let prev_state = prev.state();
    if prev_state == TaskState::Running {
        prev.set_state(TaskState::Runnable);
    }

    // ── Étape 3 : ASM context switch ─────────────────────────────────────────
    // CR3 switch uniquement si les espaces d'adressage diffèrent (KPTI-aware).
    let new_cr3 = if prev.cr3 != next.cr3 { next.cr3 } else { 0 };

    // SAFETY: prev.kernel_rsp et next.kernel_rsp pointent vers des stacks kernel
    // valides, alloués au boot et jamais libérés pendant la durée de vie du thread.
    // context_switch_asm garantit la sauvegarde complète des callee-saved ABI.
    context_switch_asm(
        &mut prev.kernel_rsp as *mut u64,
        next.kernel_rsp,
        new_cr3,
    );

    // ─────────────────────────────────────────────────────────────────────────
    // ──── À PARTIR D'ICI : on est dans le contexte de `next` ────────────────
    // (context_switch_asm a restauré la pile et les registres de `next`)
    // ─────────────────────────────────────────────────────────────────────────

    // ── Étape 4 : Post-switch côté `next` ────────────────────────────────────
    // Marquer `next` comme Running.
    next.set_state(TaskState::Running);

    // RÈGLE SWITCH-02 (suite) : Invalider FPU_LOADED pour `next`.
    // La FPU sera restaurée lazily quand `next` utilise une instruction FP (#NM).
    // Cela évite une XRSTOR coûteuse si `next` n'utilise pas la FPU cette tranche.
    next.set_fpu_loaded(false);

    // Vérifier signal pending (lecture pure, pas de livraison).
    // La livraison effective sera faite par arch/syscall.rs au retour userspace.
    let _sig = check_signal_pending(next); // résultat ignoré ici — arch/ s'en occupe

    // Instrumentation : l'appelant (tick handler) incrémente les stats switch.
}

// ─────────────────────────────────────────────────────────────────────────────
// Yield volontaire
// ─────────────────────────────────────────────────────────────────────────────

/// Yield volontaire du thread courant.
/// Place le thread courant en fin de file CFS avant d'appeler context_switch.
///
/// Appelé depuis : sys_sched_yield(), mutex_lock() (contention), condvar_wait().
pub unsafe fn schedule_yield(
    rq:      &mut crate::scheduler::core::runqueue::PerCpuRunQueue,
    current: &mut ThreadControlBlock,
) {
    use core::ptr::NonNull;
    use crate::scheduler::core::pick_next::{pick_next_task, PickResult};

    // Ré-enqueuer le courant AVANT de choisir le suivant (round-robin CFS).
    // SAFETY: current est une référence mutable valide, non nulle par construction.
    let ptr = NonNull::new_unchecked(current as *mut ThreadControlBlock);
    rq.enqueue(ptr);
    current.set_state(TaskState::Runnable);

    match pick_next_task(rq, Some(ptr)) {
        PickResult::Switch(next) => {
            // SAFETY: next provient de la run queue, toujours valide.
            let next_ref = &mut *next.as_ptr();
            // Ne pas se ré-switcher vers soi-même.
            if !core::ptr::eq(current, next_ref) {
                context_switch(current, next_ref);
            } else {
                // Retirer le thread qu'on vient d'enqueuer (c'est nous-mêmes).
                rq.remove(ptr);
                current.set_state(TaskState::Running);
            }
        }
        PickResult::KeepRunning | PickResult::GoIdle => {
            // Aucun autre thread prêt → on se retire de la queue aussi.
            rq.remove(ptr);
            current.set_state(TaskState::Running);
        }
    }
}
// ─────────────────────────────────────────────────────────────────────────────
// schedule_block — blocage du thread courant (sans ré-enfilage)
// ─────────────────────────────────────────────────────────────────────────────

/// Bloque le thread courant sans le ré-enqueuer dans la run queue.
///
/// À appeler après avoir inséré le thread dans une WaitQueue et
/// positionné son état sur `Sleeping` ou `Uninterruptible`.
/// Le thread ne sera reschedule que lorsqu'un appel à `wake_one`/`wake_all`
/// repositionnera son état sur `Runnable` ET l'enfilera de nouveau dans la RQ.
///
/// # Safety
/// - Préemption désactivée requise (PreemptGuard ou IrqGuard).
/// - `current` doit avoir son état déjà positionné sur Sleeping/Uninterruptible.
///   Ne PAS appeler si on souhaite conserver l'état Running ou Runnable.
pub unsafe fn schedule_block(
    rq:      &mut crate::scheduler::core::runqueue::PerCpuRunQueue,
    current: &mut ThreadControlBlock,
) {
    use core::ptr::NonNull;
    use crate::scheduler::core::pick_next::{pick_next_task, PickResult};

    let current_ptr = NonNull::new_unchecked(current as *mut ThreadControlBlock);

    match pick_next_task(rq, Some(current_ptr)) {
        PickResult::Switch(next) => {
            if !core::ptr::eq(current, next.as_ptr()) {
                // SAFETY: next provient de la run queue et est valide.
                context_switch(current, &mut *next.as_ptr());
            } else {
                // Seul thread disponible — impossible de bloquer réellement.
                // Remettre en Runnable + ré-enqueuer pour éviter le gel.
                current.set_state(TaskState::Runnable);
                rq.enqueue(current_ptr);
            }
        }
        PickResult::KeepRunning | PickResult::GoIdle => {
            // Aucun thread prêt → on ne peut pas bloquer, spin.
            current.set_state(TaskState::Runnable);
            rq.enqueue(current_ptr);
        }
    }
}

/// Enfile un TCB après réveil depuis WaitQueue.
/// À appeler depuis `wake_one`/`wake_all` pour que le thread soit reschedule.
///
/// # Safety
/// Préemption désactivée requise.
#[inline(always)]
pub unsafe fn wake_enqueue(
    rq:  &mut crate::scheduler::core::runqueue::PerCpuRunQueue,
    tcb: core::ptr::NonNull<ThreadControlBlock>,
) {
    use crate::scheduler::core::task::TaskState;
    (*tcb.as_ptr()).set_state(TaskState::Runnable);
    rq.enqueue(tcb);
}