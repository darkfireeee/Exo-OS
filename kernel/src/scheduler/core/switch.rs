// kernel/src/scheduler/core/switch.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// CONTEXT SWITCH — Dispatch vers switch_asm.s (Exo-OS Scheduler · Couche 1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLES (GI-02 + Corrections) :
//   RÈGLE SWITCH-01 : check_signal_pending() LIT uniquement — jamais de livraison
//   RÈGLE SWITCH-02 : Lazy FPU AVANT le switch, CR0.TS=1 APRÈS (V7-C-02)
//   RÈGLE SIGNAL-01 : scheduler/ NE connaît PAS process::signal::*
//                     Il lit seulement le flag AtomicBool signal_pending du TCB
//   RÈGLE SWITCH-ASM : switch_asm.s sauvegarde rbx,rbp,r12-r15 UNIQUEMENT (V7-C-02)
//                      PAS de MXCSR/FCW — gérés par XSAVE/XRSTOR dans fpu/
//                      CR3 switché dans switch_asm AVANT restauration des registres (KPTI)
//   CORR-11 : FS/GS base sauvegardés via rdmsr/wrmsr dans context_switch()
//   V7-C-03 : TSS.RSP0 mis à jour après chaque switch (sinon IRQ sur mauvaise pile)
//   ZONE NO-ALLOC : aucune allocation dans ce chemin chaud
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicUsize, Ordering};
use super::task::{ThreadControlBlock, TaskState};
use super::preempt::MAX_CPUS;
use crate::scheduler::fpu;
use crate::arch::x86_64::{
    cpu::{features::CPU_FEATURES, msr::{self, MSR_FS_BASE, MSR_KERNEL_GS_BASE, MSR_IA32_PL0_SSP}},
    smp::percpu,
    tss,
};
use crate::memory::virt::page_table::kpti_split::user_cr3_for_cpu;

// ─────────────────────────────────────────────────────────────────────────────
// Pointeur "thread courant" par CPU — mis à jour à chaque context switch
// ─────────────────────────────────────────────────────────────────────────────

/// Tableau cross-CPU : publie le TCB courant de chaque CPU après un switch.
///
/// Contrat :
/// - lecture locale : préférer `percpu::read_current_tcb()` via GS:[0x20]
/// - lecture distante : utiliser ce tableau avec la paire Release/Acquire
/// - publication : effectuée APRÈS mise à jour des slots GS locaux
pub static CURRENT_THREAD_PER_CPU: [AtomicUsize; MAX_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_CPUS];

/// Retourne le pointeur brut vers le TCB du thread courant sur ce CPU.
///
/// Lit le slot per-CPU canonique `gs:[0x20]` initialisé par `percpu::set_current_tcb()`.
///
/// # Safety
/// Le pointeur est non-null si le scheduler est initialisé et un thread tourne.
#[inline]
pub fn current_thread_raw() -> *mut ThreadControlBlock {
    // BUG-C2A FIX: lire depuis gs:[0x20] per-CPU, pas l'index 0
    // SAFETY: GS per-CPU initialisé avant tout appel à cette fonction
    let tcb_ptr = unsafe { crate::arch::x86_64::smp::percpu::read_current_tcb() };
    tcb_ptr as *mut ThreadControlBlock
}

/// Bloque le thread courant.
///
/// À appeler APRÈS avoir inséré le thread dans une file d'attente (futex bucket,
/// IpcWaitQueue, etc.) pour garantir qu'un réveil ne sera pas manqué.
///
/// Le thread reprend depuis cet appel après que `wake_enqueue()` a été appelé
/// sur son TCB par la partie réveillante.
///
/// # Safety
/// - La préemption doit être désactivée ou l'appelant doit garantir qu'un réveil
///   ne peut pas arriver entre l'insertion dans la file et cet appel.
/// - Le thread doit avoir un mécanisme de réveil en place (waiter.woken, etc.).
pub unsafe fn block_current_thread() {
    use crate::scheduler::core::runqueue::run_queue;

    let tcb_ptr = current_thread_raw();
    if tcb_ptr.is_null() {
        // Scheduler non encore initialisé — spin court.
        for _ in 0..1_000 {
            core::hint::spin_loop();
        }
        return;
    }

    let tcb = &mut *tcb_ptr;
    let cpu_id = tcb.current_cpu();
    if (cpu_id.0 as usize) < MAX_CPUS {
        let rq = run_queue(cpu_id);
        tcb.set_state(TaskState::Sleeping);
        schedule_block(rq, tcb);
        // Le thread reprend ici après que wake_enqueue() a été appelé.
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ASM context switch — intégré via global_asm! (switch_asm.s)
//
// Inclure le fichier .s complet dans l'unité de compilation Rust évite la
// nécessité d'un build.rs/cc pour compiler un .s externe. Le linker voit
// les symboles `context_switch_asm` et `switch_to_new_thread` directement
// dans l'rlib produit.
// ─────────────────────────────────────────────────────────────────────────────
core::arch::global_asm!(include_str!("../asm/switch_asm.s"), options(att_syntax));

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
    /// - `old_kernel_rsp` : `*mut u64` pointant vers `TCB::kstack_ptr` du thread sortant
    /// - `new_kernel_rsp` : valeur du `TCB::kstack_ptr` du thread entrant
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
    tcb.has_signal_pending()
}

// ─────────────────────────────────────────────────────────────────────────────
// context_switch() — point d'entrée Rust du switch
// ─────────────────────────────────────────────────────────────────────────────

/// Effectue le context switch de `prev` vers `next`.
///
/// # Séquence (GI-02 complète)
/// 1. Lazy FPU : si `prev` a utilisé la FPU → XSAVE (sans toucher MXCSR/FCW V7-C-02).
/// 2. Sauvegarder PKRS (Intel PKS).
/// 3. Sauvegarder FS.base et user_gs_base via rdmsr (CORR-11).
/// 4. Marquer `prev` → Runnable.
/// 5. Appeler `context_switch_asm(prev_rsp_ptr, next_rsp, next_cr3)`.
///    L'ASM sauvegarde/restaure 6 callee-saved GPRs. CR3 switché si différent.
/// 6. Restaurer PKRS de `next`.
/// 7. Marquer `next` → Running. Mettre à jour CURRENT_THREAD_PER_CPU.
/// 8. Mettre CR0.TS = 1 (Lazy FPU — V7-C-02). set_fpu_loaded(false).
/// 9. Mettre à jour TSS.RSP0 ← next.kstack_ptr (V7-C-03 OBLIGATOIRE).
/// 10. Restaurer FS.base et user_gs_base de `next` via wrmsr (CORR-11).
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
    // ── Étape 1 : Lazy FPU save (RÈGLE SWITCH-02) ─────────────────────────────
    // Sauvegarder l'état FPU du thread sortant si elle était chargée (CR0.TS=0).
    // Note : CR0.TS sera remis à 1 après le switch (step 7 ci-dessous).
    if prev.fpu_loaded() {
        fpu::save_restore::xsave_current(prev);
        // FPU_LOADED reste true jusqu'à step 9 (set_fpu_loaded(false) pour next).
        // Si un IRQ arrive entre step 1 et step 5, la FPU est déjà sauvée.
    }

    // Sauvegarde PKRS (S6) si le CPU supporte PKS.
    if CPU_FEATURES.has_pks() {
        // SAFETY: accès MSR ring0, capability vérifiée via CPUID.
        prev.pkrs = unsafe { msr::read_msr(msr::MSR_IA32_PKRS) as u32 };
    }

    // ── FIX-CET-01 : Sauvegarder MSR_IA32_PL0_SSP si CET Shadow Stack actif ──
    //
    // Intel SDM Vol.1 §8.3.3 : en mode software task switch, la sauvegarde du
    // Shadow Stack Pointer est à la charge du noyau. Si CET n'est pas actif,
    // le MSR 0x6A4 n'existe pas → on court-circuite via has_cet_ss().
    if CPU_FEATURES.has_cet_ss() {
        // SAFETY: MSR 0x6A4 existant si has_cet_ss() == true. Ring 0.
        let ssp = unsafe { msr::read_msr(MSR_IA32_PL0_SSP) };
        prev.set_pl0_ssp(ssp);
    }

    // ── Étape 2 : Sauvegarder FS/GS base du thread sortant (CORR-11) ─────────
    //
    // On est en Ring 0 → SWAPGS a été effectué à l'entrée kernel.
    //   MSR_FS_BASE       (0xC0000100) = FS.base courant (TLS userspace prev)
    //   MSR_KERNEL_GS_BASE (0xC0000102) = GS.base caché (valeur userspace prev)
    //     [GS.base actuel (0xC0000101) = per-CPU kernel data — ne pas sauvegarder]
    //
    // ERREUR SILENCIEUSE S-06 : sauvegarder 0xC0000101 au lieu de 0xC0000102
    //   → TLS Ring 3 corrompu après chaque context switch entre threads différents.
    //
    // SAFETY: rdmsr en Ring 0 sur MSR valides et supportés (x86_64 requis).
    prev.fs_base       = unsafe { msr::read_msr(MSR_FS_BASE) };
    prev.user_gs_base  = unsafe { msr::read_msr(MSR_KERNEL_GS_BASE) };

    // ── Étape 3 : Transition d'état de prev ──────────────────────────────────
    // Si le thread sortant était Running → il redevient Runnable (sera ré-enfilé).
    // Si il était dans un état bloquant (Sleeping, Uninterruptible) → on ne change pas.
    let prev_state = prev.state();
    if prev_state == TaskState::Running {
        prev.set_state(TaskState::Runnable);
    }

    // ── Étape 4 : ASM context switch ─────────────────────────────────────────
    // CR3 switch uniquement si les espaces d'adressage diffèrent (KPTI-aware).
    let new_cr3 = if prev.cr3_phys != next.cr3_phys { next.cr3_phys } else { 0 };

    // SAFETY: prev.kstack_ptr et next.kstack_ptr pointent vers des stacks kernel
    // valides, alloués au boot et jamais libérés pendant la durée de vie du thread.
    // context_switch_asm garantit la sauvegarde complète des callee-saved ABI.
    context_switch_asm(
        &mut prev.kstack_ptr as *mut u64,
        next.kstack_ptr,
        new_cr3,
    );

    // ─────────────────────────────────────────────────────────────────────────
    // ──── À PARTIR D'ICI : on est dans le contexte de `next` ────────────────
    // (context_switch_asm a restauré la pile et les registres de `next`)
    // ─────────────────────────────────────────────────────────────────────────

    // FIX-KPTI-01 : rafraîchir le slot CR3 per-CPU après le switch.
    // Sans cette mise à jour, kpti_switch_to_user/kernel peut relire un couple
    // stale sur ce CPU après migration/changement d'espace d'adressage.
    if crate::arch::x86_64::spectre::kpti::kpti_enabled() {
        let cpu_id = percpu::current_cpu_id() as usize;
        let user_cr3 = user_cr3_for_cpu(cpu_id).unwrap_or(next.cr3_phys);
        crate::arch::x86_64::spectre::kpti::set_current_cr3(next.cr3_phys, user_cr3);
    }

    // Restauration PKRS (S6) côté thread entrant.
    if CPU_FEATURES.has_pks() {
        // SAFETY: accès MSR ring0, capability vérifiée via CPUID.
        unsafe { msr::write_msr(msr::MSR_IA32_PKRS, next.pkrs as u64) };
    }

    // ── FIX-CET-01 : Restaurer MSR_IA32_PL0_SSP de `next` si CET actif ───────
    //
    // On est maintenant dans le contexte de `next`. Restaurer son SSP Ring 0
    // avant tout retour vers userspace. Si next n'a jamais utilisé CET
    // (pl0_ssp() == 0), écrire 0 est safe — désactive le shadow stack pour ce
    // thread jusqu'à activation explicite via ExoCage.
    if CPU_FEATURES.has_cet_ss() {
        let ssp = next.pl0_ssp();
        // SAFETY: MSR 0x6A4 existant si has_cet_ss(), Ring 0, valeur par-thread.
        unsafe { msr::write_msr(MSR_IA32_PL0_SSP, ssp) };
    }

    // ── Étape 5 : Post-switch côté `next` ────────────────────────────────────
    // Marquer `next` comme Running.
    next.set_state(TaskState::Running);

    // Mettre à jour d'abord les slots GS locaux : c'est la source de vérité
    // pour le CPU courant pendant les entrées syscall/exception.
    percpu::set_current_tcb(next as *mut ThreadControlBlock);

    // P1-5: rafraîchir le slot GS:[0x00] avec la pile kernel du thread entrant.
    // L'entrée syscall (`mov rsp, gs:[0x00]`) doit toujours pointer sur le
    // contexte kernel du thread courant pour éviter toute corruption silencieuse.
    unsafe { percpu::set_kernel_rsp(next.kstack_ptr); }

    // Publier ensuite l'état `next` aux autres CPUs.
    core::sync::atomic::fence(Ordering::SeqCst);
    // SAFETY: cpu() est monotone pendant l'exécution d'un thread sur ce CPU.
    CURRENT_THREAD_PER_CPU[next.current_cpu().0 as usize]
        .store(next as *mut ThreadControlBlock as usize, Ordering::Release);

    // ── Étape 6 : CR0.TS = 1 — Lazy FPU (V7-C-02) ───────────────────────────
    // Déclenche #NM si `next` utilise une instruction FPU/SSE sans l'avoir
    // chargée. Le handler #NM (lazy.rs) fera XRSTOR et remettra CR0.TS=0.
    // SAFETY: cr0_set_ts modifie CR0.TS en Ring 0 — opération sûre au boot
    // et lors de chaque context switch.
    unsafe { fpu::lazy::cr0_set_ts(); }
    next.set_fpu_loaded(false);

    // ── Étape 7 : TSS.RSP0 obligatoire (V7-C-03) ─────────────────────────────
    // Met à jour TSS.RSP0 avec le haut de pile kernel de `next`.
    //
    // ERREUR SILENCIEUSE S-08 : si oublié, la prochaine interruption Ring 3→0
    // empilera sur la pile de l'ANCIEN thread → corruption silencieuse.
    //
    // SAFETY: next.current_cpu().0 < MAX_CPUS et next.kstack_ptr est valide.
    unsafe {
        tss::update_rsp0(next.current_cpu().0 as usize, next.kstack_ptr);
    }

    // ── Étape 8 : Restaurer FS/GS base de `next` (CORR-11) ──────────────────
    //
    // Restaurer FS.base (TLS userspace) et user GS.base (valeur Ring 3).
    // Écrire user_gs_base dans MSR_KERNEL_GS_BASE (0xC0000102) : lors du
    // SWAPGS à IRETQ vers Ring 3, cette valeur deviendra GS.base.
    //
    // SAFETY: wrmsr en Ring 0 sur MSR valides et supportés.
    unsafe {
        msr::write_msr(MSR_FS_BASE, next.fs_base);
        msr::write_msr(MSR_KERNEL_GS_BASE, next.user_gs_base);
    }

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
