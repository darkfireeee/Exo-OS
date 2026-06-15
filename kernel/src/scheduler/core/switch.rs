// kernel/src/scheduler/core/switch.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// CONTEXT SWITCH — Dispatch vers switch_asm.s (Exo-OS Scheduler · Couche 1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLES (GI-02 + Corrections) :
//   RÈGLE SWITCH-01 : check_signal_pending() LIT uniquement — jamais de livraison
//   RÈGLE SWITCH-02 : Lazy FPU AVANT le switch, CR0.TS=1 AVANT l'ASM (TLA-01)
//   RÈGLE SIGNAL-01 : scheduler/ NE connaît PAS process::signal::*
//                     Il lit seulement le flag AtomicBool signal_pending du TCB
//   RÈGLE SWITCH-ASM : switch_asm.s sauvegarde rbx,rbp,r12-r15 UNIQUEMENT (V7-C-02)
//                      PAS de MXCSR/FCW — gérés par XSAVE/XRSTOR dans fpu/
//                      CR3 switché dans switch_asm AVANT restauration des registres (KPTI)
//   CORR-11 : FS/GS base sauvegardés via rdmsr/wrmsr dans context_switch()
//   V7-C-03 : TSS.RSP0 mis à jour après chaque switch (sinon IRQ sur mauvaise pile)
//   ZONE NO-ALLOC : aucune allocation récurrente dans ce chemin chaud
//                   (shadow KPTI construit une seule fois par TCB si requis)
// ═══════════════════════════════════════════════════════════════════════════════

use super::preempt::MAX_CPUS;
use super::task::{CpuId, SchedPolicy, TaskState, ThreadControlBlock};
use crate::arch::x86_64::{
    cpu::{
        features::cpu_features_or_none,
        msr::{self, MSR_FS_BASE, MSR_IA32_PL0_SSP, MSR_KERNEL_GS_BASE,
               MSR_IA32_PRED_CMD, PRED_CMD_IBPB},
        tsc,
    },
    smp::percpu,
    tss,
};
use crate::memory::core::PhysAddr;
use crate::memory::virt::page_table::kpti_split::{build_user_shadow_pml4, user_cr3_for_cpu};
use crate::scheduler::fpu;
use core::sync::atomic::{AtomicUsize, Ordering};

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

/// Hook optionnel appelé sur le thread sortant avant la transition d'état.
///
/// Les couches supérieures l'installent au boot ; le scheduler garde seulement
/// un pointeur de fonction pour éviter une dépendance directe sur security/.
pub type ContextSwitchOutHook = fn(&ThreadControlBlock);

static CONTEXT_SWITCH_OUT_HOOK: AtomicUsize = AtomicUsize::new(0);
#[cfg(all(target_arch = "x86_64", debug_assertions))]
static IDLE_SCHED_TRACE_COUNT: AtomicUsize = AtomicUsize::new(0);

#[cfg(all(target_arch = "x86_64", debug_assertions))]
#[inline]
fn idle_sched_trace(message: &[u8]) {
    if false && IDLE_SCHED_TRACE_COUNT.fetch_add(1, Ordering::Relaxed) < 32 {
        crate::arch::x86_64::terminal::debug_write(message);
    }
}

#[cfg(not(all(target_arch = "x86_64", debug_assertions)))]
#[inline]
fn idle_sched_trace(_message: &[u8]) {}

#[inline]
pub fn install_context_switch_out_hook(hook: ContextSwitchOutHook) {
    CONTEXT_SWITCH_OUT_HOOK.store(hook as usize, Ordering::Release);
}

/// Retourne le pointeur brut vers le TCB du thread courant sur ce CPU.
///
/// Lit le slot per-CPU canonique `gs:[0x20]` initialisé par `percpu::set_current_tcb()`.
///
/// # Safety
/// Le pointeur est non-null si le scheduler est initialisé et un thread tourne.
#[inline]
pub fn current_thread_raw() -> *mut ThreadControlBlock {
    // BUG-C2A FIX: lire depuis gs:[0x20] per-CPU, pas l'index 0.
    // Pendant les toutes premières phases de boot, le slot GS peut encore être
    // nul alors que la publication canonique cross-CPU a déjà eu lieu.
    // On retombe alors sur CURRENT_THREAD_PER_CPU[cpu] pour éviter un faux null
    // pendant la couture BSP/AP.
    // SAFETY: read_current_tcb() lit le pointeur TCB depuis gs:[0x20] du CPU
    // courant via un accès MSR/segment Ring 0. Aucune déréférence ici — on ne
    // fait que convertir le pointeur publié en usize pour tester sa nullité.
    let gs_tcb = unsafe { crate::arch::x86_64::smp::percpu::read_current_tcb() as usize };
    if gs_tcb != 0 {
        return gs_tcb as *mut ThreadControlBlock;
    }

    let cpu_id = percpu::current_cpu_id() as usize;
    if cpu_id >= MAX_CPUS {
        return core::ptr::null_mut();
    }

    CURRENT_THREAD_PER_CPU[cpu_id].load(Ordering::Acquire) as *mut ThreadControlBlock
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

    debug_assert!(
        crate::scheduler::core::preempt::PreemptGuard::depth() == 0,
        "block_current_thread: appelé avec PreemptGuard actif"
    );

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
        match tcb.state() {
            TaskState::Runnable => {
                // SAFETY: tcb provient de current_thread_raw() (non-null vérifié)
                // et rq est la run-queue du CPU possédant ce thread. La préemption
                // est désactivée par le contrat de block_current_thread() (unsafe fn).
                unsafe {
                    finish_preblock_wake(rq, tcb);
                }
                return;
            }
            TaskState::Running => return,
            TaskState::Sleeping
            | TaskState::Uninterruptible
            | TaskState::Stopped
            | TaskState::Dead => {}
            state => {
                debug_assert!(
                    false,
                    "block_current_thread: état inattendu {:?}; l'appelant doit transitionner le thread avant blocage",
                    state
                );
                return;
            }
        }
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
    /// Sauvegarde/restaure uniquement les registres callee-saved ABI
    /// (rbx, rbp, r12-r15) et la pile kernel du thread sortant. L'état FPU
    /// (XSAVE/XRSTOR, MXCSR et x87 FCW compris) est géré par `scheduler::fpu`.
    /// Le CR3 est switché si `new_cr3 != 0`.
    ///
    /// # Arguments (System V ABI)
    /// - `old_kernel_rsp` : `*mut u64` pointant vers `TCB::kstack_ptr` du thread sortant
    /// - `new_kernel_rsp` : valeur du `TCB::kstack_ptr` du thread entrant
    /// - `new_cr3`        : registre CR3 du thread entrant (0 = pas de switch CR3)
    fn context_switch_asm(old_kernel_rsp: *mut u64, new_kernel_rsp: u64, new_cr3: u64);
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
/// 2. Restaurer l'etat FPU/SIMD entrant et garder CR0.TS clair pour Ring 3.
/// 3. Sauvegarder PKRS (Intel PKS).
/// 4. Sauvegarder CET PL0_SSP si shadow stack actif.
/// 5. Sauvegarder FS.base et user_gs_base via rdmsr (CORR-11).
/// 6. Marquer `prev` → Runnable.
/// 7. Calculer `next_cr3` et comptabiliser le runtime de `prev`.
/// 8. Appeler `context_switch_asm(prev_rsp_ptr, next_rsp, next_cr3)`.
///    L'ASM sauvegarde/restaure 6 callee-saved GPRs. CR3 switché si différent.
/// 9. Restaurer PKRS de `next`.
/// 10. Restaurer CET PL0_SSP de `next` si shadow stack actif.
/// 11. Marquer `next` → Running, puis mettre à jour pile kernel/TSS.RSP0.
/// 12. Restaurer FS.base puis user_gs_base, enfin publier GS current_tcb.
///
/// # Sécurité
/// - Appelé avec préemption désactivée (IrqGuard ou PreemptGuard).
/// - `prev` et `next` DOIVENT être des pointeurs valides.
///
/// # RÈGLE ABSOLUE
/// Cette fonction NE doit JAMAIS appeler `process::signal::*`.
/// Elle ne fait que lire `signal_pending` via `check_signal_pending()`.
pub unsafe fn context_switch(prev: &mut ThreadControlBlock, next: &mut ThreadControlBlock) {
    let features = cpu_features_or_none();
    let has_pks    = features.map_or(false, |cpu| cpu.has_pks());
    let has_cet_ss = features.map_or(false, |cpu| cpu.has_cet_ss());
    // FIX-IBPB (Security_Audit_Passe2 §B-01) : IBPB (Indirect Branch Predictor Barrier)
    // doit être émis lors d'un context-switch cross-processus pour prévenir Spectre v2.
    // Gated sur :
    //   1. CPU supportant IBPB (has_ibpb via CPUID SPEC_CTRL ou IBPB_AMD)
    //   2. Changement de processus (prev.pid ≠ next.pid) — les switch intra-process
    //      (threads du même processus) n'ont pas besoin d'IBPB.
    //   3. Processus Ring3 uniquement (pid != 0 ; les kthreads partagent Ring0 BTB).
    let ibpb_needed = features.map_or(false, |cpu| cpu.has_ibpb())
        && prev.pid != next.pid
        && next.pid.0 != 0;
    if ibpb_needed {
        // SAFETY: MSR_IA32_PRED_CMD (0x49) est garanti présent par has_ibpb().
        // L'écriture est atomique et visible sur le CPU local uniquement.
        // PRED_CMD_IBPB (bit 0) vide le prédicteur de branchements indirect.
        unsafe { msr::write_msr(MSR_IA32_PRED_CMD, PRED_CMD_IBPB) };
    }

    // ── Etape 1 : sauvegarde FPU/SIMD du thread sortant ──────────────────────
    // Les binaires Ring3 Rust utilisent SSE pour des copies de structures
    // meme sans flottants explicites. Un mode lazy qui repose sur #NM rend le
    // premier shell fragile; on sauvegarde/restaure donc au switch.
    if prev.fpu_loaded() {
        fpu::save_restore::xsave_current(prev);
    }
    prev.set_fpu_loaded(false);
    // SAFETY: Ring 0, préemption désactivée (contrat de context_switch, unsafe fn).
    // cr0_clear_ts() efface CR0.TS pour autoriser SSE/AVX ; xrstor_for(next)
    // restaure l'état FPU de `next` depuis sa zone XSAVE par-thread alignée.
    unsafe {
        fpu::lazy::cr0_clear_ts();
        if next.is_kthread() {
            next.set_fpu_loaded(false);
        } else {
            fpu::save_restore::xrstor_for(next);
        }
    }

    // ── Étape 3 : Sauvegarde PKRS (S6) si le CPU supporte PKS ───────────────
    if has_pks {
        // SAFETY: accès MSR ring0, capability vérifiée via CPUID.
        prev.pkrs = unsafe { msr::read_msr(msr::MSR_IA32_PKRS) as u32 };
    }

    // ── Étape 4 : Sauvegarder MSR_IA32_PL0_SSP si CET Shadow Stack actif ────
    //
    // Intel SDM Vol.1 §8.3.3 : en mode software task switch, la sauvegarde du
    // Shadow Stack Pointer est à la charge du noyau. Si CET n'est pas actif,
    // le MSR 0x6A4 n'existe pas → on court-circuite via has_cet_ss().
    if has_cet_ss {
        // SAFETY: MSR 0x6A4 existant si has_cet_ss() == true. Ring 0.
        let ssp = unsafe { msr::read_msr(MSR_IA32_PL0_SSP) };
        prev.set_pl0_ssp(ssp);
    }

    // ── Étape 5 : Sauvegarder FS/GS base du thread sortant (CORR-11) ─────────
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
    prev.fs_base = unsafe { msr::read_msr(MSR_FS_BASE) };
    prev.user_gs_base = unsafe { msr::read_msr(MSR_KERNEL_GS_BASE) };

    let hook = CONTEXT_SWITCH_OUT_HOOK.load(Ordering::Acquire);
    if hook != 0 {
        // SAFETY: hook != 0 garantit qu'un pointeur de fonction valide a été
        // publié via register_context_switch_out_hook() (signature
        // ContextSwitchOutHook). La transmutation usize→fn pointer est correcte
        // car l'adresse provient d'un `fn` du même ABI stockée atomiquement.
        let hook_fn: ContextSwitchOutHook = unsafe { core::mem::transmute(hook) };
        hook_fn(prev);
    }

    // ── Étape 6 : Transition d'état de prev ──────────────────────────────────
    // Si le thread sortant était Running → il redevient Runnable (sera ré-enfilé).
    // Si il était dans un état bloquant (Sleeping, Uninterruptible) → on ne change pas.
    let prev_state = prev.state();
    if prev_state == TaskState::Running {
        prev.set_state(TaskState::Runnable);
    }

    // ── Étape 7 : préparer CR3 + runtime, puis Étape 8 : ASM context switch ─
    // FIX-CR3-RESYNC : on recharge TOUJOURS le CR3 du thread cible (s'il en a un,
    // i.e. thread user). L'ancienne optim « skip si prev.cr3 == next.cr3 »
    // supposait que le CR3 matériel valait déjà prev.cr3_phys — faux dès qu'un
    // chemin (execve/handoff/fork-trampoline) change le CR3 matériel sans repasser
    // par ce switch. Résultat observé : init (PID 1) s'exécutait sur un CR3 ≠ de
    // son `tcb.cr3_phys`, donc le demand-paging mappait dans la mauvaise table →
    // boucle de #PF instruction-fetch. `tcb.cr3_phys` est la source de vérité
    // (mise à jour par execve) ; le matériel doit la suivre à chaque switch.
    // new_cr3 == 0 ⇒ l'ASM ne recharge pas (threads kernel sans espace user).
    let new_cr3 = if next.cr3_phys != 0 {
        next.cr3_phys
    } else {
        0
    };

    // Comptabiliser le temps réellement passé en Running par `prev`.
    let now_tsc = tsc::read_tsc();
    let cpu_idx = percpu::current_cpu_id() as usize;
    if cpu_idx < MAX_CPUS {
        // SAFETY: cpu_idx < MAX_CPUS borne l'accès au tableau per-CPU statique ;
        // chaque cœur n'écrit que son propre slot (pas de partage mutable), donc
        // la &mut est exclusive sur ce CPU. Préemption désactivée.
        let cpu_data = unsafe { percpu::per_cpu_mut(cpu_idx) };
        let last = cpu_data.last_switch_tsc;
        if last != 0 {
            let delta_ns = tsc::tsc_cycles_to_ns(now_tsc.wrapping_sub(last));
            prev.run_time_acc = prev.run_time_acc.saturating_add(delta_ns);
            match prev.policy {
                SchedPolicy::Normal | SchedPolicy::Batch => {
                    prev.advance_vruntime(delta_ns, prev.priority.cfs_weight());
                }
                _ => {}
            }
        }
    }

    // Publier `next` AVANT le saut ASM.
    //
    // Un thread jamais schedule ne revient pas a l'instruction suivant
    // `context_switch_asm`: son premier `ret` saute vers son trampoline de
    // demarrage. Les etats per-CPU doivent donc deja designer `next` avant de
    // charger sa pile kernel.
    next.assign_cpu(CpuId(cpu_idx as u32));

    let next_kstack_top = next.kstack_top();

    if crate::arch::x86_64::spectre::kpti::kpti_enabled() {
        let mut user_cr3 = next.kpti_user_cr3();
        if user_cr3 == 0 && next.pid.0 != 0 && next.cr3_phys != 0 {
            let trampoline_phys = PhysAddr::new(crate::arch::x86_64::smp::init::TRAMPOLINE_PHYS);
            // SAFETY: construit la PML4 shadow KPTI pour `next` à partir de son
            // CR3 kernel (non-nul, pid != 0 vérifiés ci-dessus), du trampoline
            // SMP physique et du sommet de pile kernel valide. Ring 0, mappings
            // contrôlés par le walker de tables ; échec → halt CPU.
            user_cr3 = unsafe {
                match build_user_shadow_pml4(
                    PhysAddr::new(next.cr3_phys),
                    cpu_idx,
                    trampoline_phys,
                    next_kstack_top,
                ) {
                    Ok(cr3) => cr3.as_u64(),
                    Err(_err) => {
                        crate::arch::x86_64::terminal::debug_write(
                            b"KPTI: echec construction user shadow\n",
                        );
                        crate::arch::x86_64::halt_cpu();
                    }
                }
            };
            next.set_kpti_user_cr3(user_cr3);
        }
        if user_cr3 == 0 {
            user_cr3 = user_cr3_for_cpu(cpu_idx).unwrap_or(next.cr3_phys);
        }
        if user_cr3 == 0 {
            crate::arch::x86_64::terminal::debug_write(b"KPTI: user CR3 absent\n");
            crate::arch::x86_64::halt_cpu();
        }
        crate::arch::x86_64::spectre::kpti::set_current_cr3(next.cr3_phys, user_cr3);
    }

    if has_pks {
        // SAFETY: accès MSR ring0, capability vérifiée via CPUID.
        unsafe { msr::write_msr(msr::MSR_IA32_PKRS, next.pkrs as u64) };
    }

    if has_cet_ss {
        let ssp = next.pl0_ssp();
        // SAFETY: MSR 0x6A4 existant si has_cet_ss(), Ring 0, valeur par-thread.
        unsafe { msr::write_msr(MSR_IA32_PL0_SSP, ssp) };
    }

    next.set_state(TaskState::Running);
    next.switch_count = next.switch_count.wrapping_add(1);

    // SAFETY: next_kstack_top est le sommet de la pile kernel de `next` (alloué
    // au spawn). On publie ce RSP0 dans l'état per-CPU et la TSS du cœur courant
    // (cpu_idx < MAX_CPUS) pour que la prochaine entrée Ring3→Ring0 utilise la
    // bonne pile. Ring 0, préemption désactivée.
    unsafe {
        percpu::set_kernel_rsp(next_kstack_top);
        tss::update_rsp0(cpu_idx, next_kstack_top);
    }

    // SAFETY: wrmsr en Ring 0 sur MSR valides et supportés.
    unsafe {
        msr::write_msr(MSR_FS_BASE, next.fs_base);
        msr::write_msr(MSR_KERNEL_GS_BASE, next.user_gs_base);
    }

    // Publier seulement après FS puis user-GS, conformément à ContextSwitch.tla.
    core::sync::atomic::fence(Ordering::SeqCst);
    percpu::set_current_tcb(next as *mut ThreadControlBlock);

    if cpu_idx < MAX_CPUS {
        // SAFETY: cpu_idx < MAX_CPUS ; accès exclusif au slot per-CPU du cœur
        // courant (pas de partage mutable cross-CPU). Ring 0, préemption off.
        let cpu_data = unsafe { percpu::per_cpu_mut(cpu_idx) };
        cpu_data.ctx_switch_count = cpu_data.ctx_switch_count.wrapping_add(1);
        cpu_data.last_switch_tsc = tsc::read_tsc();
        CURRENT_THREAD_PER_CPU[cpu_idx]
            .store(next as *mut ThreadControlBlock as usize, Ordering::Release);
    }

    // SAFETY: prev.kstack_ptr et next.kstack_ptr pointent vers des stacks kernel
    // valides, alloues au boot et jamais liberes pendant la duree de vie du thread.
    // context_switch_asm garantit la sauvegarde complète des callee-saved ABI.
    if next.is_kthread() {
        idle_sched_trace(b"context_switch: ->kthread\n");
    }
    if next.pid.0 == 1 && next.cr3_phys != 0 {
        idle_sched_trace(b"context_switch: ->init-user\n");
    }
    context_switch_asm(&mut prev.kstack_ptr as *mut u64, next.kstack_ptr, new_cr3);
}

// ─────────────────────────────────────────────────────────────────────────────
// Yield volontaire
// ─────────────────────────────────────────────────────────────────────────────

/// Yield volontaire du thread courant.
/// Place le thread courant en fin de file CFS avant d'appeler context_switch.
///
/// Appelé depuis : sys_sched_yield(), mutex_lock() (contention), condvar_wait().
pub unsafe fn schedule_yield(
    rq: &mut crate::scheduler::core::runqueue::PerCpuRunQueue,
    current: &mut ThreadControlBlock,
) -> bool {
    use crate::scheduler::core::pick_next::{pick_next_task, PickResult};
    use crate::scheduler::core::preempt::IrqGuard;
    use core::ptr::NonNull;

    let ptr = NonNull::new_unchecked(current as *mut ThreadControlBlock);
    let (selected_next, irq_flags) = {
        let irq = IrqGuard::new();

        // Ré-enqueuer le courant AVANT de choisir le suivant (round-robin CFS).
        // SAFETY: current est une référence mutable valide, non nulle par construction.
        if current.is_queued() {
            let _ = rq.remove(ptr);
            current.clear_queued();
        }
        current.set_state(TaskState::Runnable);
        if !rq.enqueue(ptr) {
            current.set_state(TaskState::Running);
            return false;
        }

        let selected = match pick_next_task(rq, None) {
            PickResult::Switch(next) if next != ptr => Some(next),
            PickResult::Switch(_) => {
                // Le courant a été choisi puis retiré de la run queue. Si un
                // autre thread est prêt, on le prend et on remet le courant en
                // attente derrière lui; sinon le yield devient un no-op.
                let replacement = if rq.nr_running_usize() > 0 {
                    match pick_next_task(rq, None) {
                        PickResult::Switch(next) if next != ptr => Some(next),
                        _ => None,
                    }
                } else {
                    None
                };

                if let Some(next) = replacement {
                    if rq.enqueue(ptr) {
                        Some(next)
                    } else {
                        current.set_state(TaskState::Running);
                        rq.enqueue(next);
                        None
                    }
                } else {
                    current.set_state(TaskState::Running);
                    None
                }
            }
            PickResult::KeepRunning | PickResult::GoIdle => {
                let _ = rq.remove(ptr);
                current.set_state(TaskState::Running);
                None
            }
        };

        if selected.is_some() {
            let flags = irq.release_keep_irqs_disabled();
            (selected, Some(flags))
        } else {
            (None, None)
        }
    };

    let Some(next) = selected_next else {
        return false;
    };
    let irq_flags = irq_flags.unwrap_or(0);

    // SAFETY: next provient de la run queue, toujours valide.
    let next_ref = &mut *next.as_ptr();
    if !core::ptr::eq(current, next_ref) {
        context_switch(current, next_ref);
        // SAFETY: restaure le flag IF capturé par IrqGuard à l'entrée de la
        // section critique ; irq_flags est la valeur RFLAGS sauvée localement, Ring 0.
        unsafe {
            IrqGuard::restore_irq_flags(irq_flags);
        }
        true
    } else {
        // SAFETY: restaure le flag IF capturé par IrqGuard à l'entrée de la
        // section critique ; irq_flags est la valeur RFLAGS sauvée localement, Ring 0.
        unsafe {
            IrqGuard::restore_irq_flags(irq_flags);
        }
        current.set_state(TaskState::Running);
        false
    }
}

unsafe fn schedule_current(force: bool) -> bool {
    let tcb_ptr = current_thread_raw();
    if tcb_ptr.is_null() {
        return false;
    }

    let current = &mut *tcb_ptr;
    let cpu_id = current.current_cpu();
    if (cpu_id.0 as usize) >= MAX_CPUS {
        return false;
    }

    let rq = crate::scheduler::core::runqueue::run_queue(cpu_id);
    let should_schedule = if force {
        // Un yield coopératif est une demande explicite de sélection. Ne pas
        // le conditionner à nr_running: ce compteur peut être transitoirement
        // en retard sur la structure CFS/RT pendant les chemins fork/IPC.
        true
    } else {
        current.need_resched()
    };
    if !should_schedule {
        return false;
    }

    let _ = current.take_need_resched();
    schedule_yield(rq, current)
}

/// Honore un `NEED_RESCHED` posé par le tick timer ou une IPI avant de revenir
/// en userspace.
///
/// Le helper est volontairement petit : il ne déclenche un switch que si le TCB
/// courant demande explicitement une préemption.
pub unsafe fn schedule_current_if_needed() -> bool {
    schedule_current(false)
}

/// Point de rescheduling coopératif pour les syscalls qui attendent en boucle
/// courte tant que les vraies queues de sommeil ne sont pas encore câblées.
pub unsafe fn cooperative_reschedule() -> bool {
    schedule_current(true)
}

/// Une tentative de scheduling depuis le thread idle de boot du CPU courant.
///
/// `kernel_main` arrive ici sur le stack de boot BSP, avec un TCB idle deja
/// publie comme `current_tcb`. La fin de boot ne doit pas faire `cli; hlt`,
/// sinon les kthreads deja enfilees et les futurs processus utilisateur ne
/// peuvent jamais prendre le CPU. Ce helper choisit un thread runnable, relache
/// la section preempt avant le switch, puis bascule depuis le TCB idle courant.
///
/// Retourne `true` si un context switch a ete tente.
///
/// # Safety
/// Le scheduler et la run queue du CPU doivent etre initialises, et le TCB
/// courant doit etre le TCB idle publie pour ce CPU ou un TCB kernel valide.
pub unsafe fn schedule_idle_cpu_once(cpu_id: CpuId) -> bool {
    use crate::scheduler::core::pick_next::{pick_next_task, PickResult};
    use crate::scheduler::core::preempt::IrqGuard;
    use crate::scheduler::core::runqueue::run_queue;
    use core::ptr::NonNull;

    let current_ptr = current_thread_raw();
    let Some(current_nn) = NonNull::new(current_ptr) else {
        idle_sched_trace(b"idle_sched: no current\n");
        return false;
    };
    idle_sched_trace(b"idle_sched: enter\n");

    let (selected_next, irq_flags) = {
        let irq = IrqGuard::new();
        let rq = run_queue(cpu_id);
        let current = &mut *current_nn.as_ptr();

        if rq.nr_running_usize() == 0 && !current.need_resched() {
            idle_sched_trace(b"idle_sched: empty\n");
            return false;
        }
        let _ = current.take_need_resched();

        let selected = match pick_next_task(rq, Some(current_nn)) {
            PickResult::Switch(next) if next != current_nn => {
                idle_sched_trace(b"idle_sched: picked\n");
                Some(next)
            }
            PickResult::Switch(_) | PickResult::KeepRunning | PickResult::GoIdle => {
                idle_sched_trace(b"idle_sched: no next\n");
                None
            }
        };

        if selected.is_some() {
            let flags = irq.release_keep_irqs_disabled();
            (selected, Some(flags))
        } else {
            (None, None)
        }
    };

    let Some(next) = selected_next else {
        return false;
    };
    let irq_flags = irq_flags.unwrap_or(0);

    let current = &mut *current_nn.as_ptr();
    idle_sched_trace(b"idle_sched: switch\n");
    context_switch(current, &mut *next.as_ptr());
    // SAFETY: restaure le flag IF capturé par IrqGuard à l'entrée de la
    // section critique ; irq_flags est la valeur RFLAGS sauvée localement, Ring 0.
    unsafe {
        IrqGuard::restore_irq_flags(irq_flags);
    }
    true
}
// ─────────────────────────────────────────────────────────────────────────────
// schedule_block — blocage du thread courant (sans ré-enfilage)
// ─────────────────────────────────────────────────────────────────────────────

/// Termine une course de réveil arrivée avant le vrai blocage.
///
/// Le protocole sleep/wait est en deux phases : l'appelant publie d'abord le
/// thread comme `Sleeping`, puis appelle `schedule_block()`. Un timer ou un
/// waker peut légalement faire `Sleeping -> Runnable` et l'enfiler entre ces
/// deux opérations. Comme le thread tourne encore sur ce CPU, il faut retirer
/// l'entrée de runqueue et revenir à `Running` au lieu de bloquer.
///
/// # Safety
/// `current` doit être le TCB courant du CPU associé à `rq`.
pub unsafe fn finish_preblock_wake(
    rq: &mut crate::scheduler::core::runqueue::PerCpuRunQueue,
    current: &mut ThreadControlBlock,
) {
    use crate::scheduler::core::preempt::IrqGuard;
    use core::ptr::NonNull;

    let current_ptr = NonNull::new_unchecked(current as *mut ThreadControlBlock);
    let _irq = IrqGuard::new();
    let _ = rq.remove(current_ptr);
    current.set_state(TaskState::Running);
}

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
    rq: &mut crate::scheduler::core::runqueue::PerCpuRunQueue,
    current: &mut ThreadControlBlock,
) {
    use crate::scheduler::core::pick_next::{pick_next_task, PickResult};
    use crate::scheduler::core::preempt::IrqGuard;
    use core::ptr::NonNull;

    let current_ptr = NonNull::new_unchecked(current as *mut ThreadControlBlock);
    match current.state() {
        TaskState::Runnable => {
            finish_preblock_wake(rq, current);
            return;
        }
        TaskState::Running => return,
        TaskState::Sleeping | TaskState::Uninterruptible | TaskState::Stopped | TaskState::Dead => {
        }
        _ => return,
    }
    let mut idle_thread = match rq.idle_thread {
        Some(idle) => Some(idle),
        None => {
            let recovered = crate::scheduler::core::boot_idle::published_boot_idle(rq.cpu.0);
            if let Some(idle) = recovered {
                rq.set_idle_thread(idle);
                if rq.current.is_none() {
                    rq.current = Some(idle);
                }
            }
            recovered
        }
    };

    if idle_thread.is_none() {
        for _ in 0..1024 {
            if let Some(idle) = crate::scheduler::core::boot_idle::published_boot_idle(rq.cpu.0) {
                rq.set_idle_thread(idle);
                if rq.current.is_none() {
                    rq.current = Some(idle);
                }
                idle_thread = Some(idle);
                break;
            }
            core::hint::spin_loop();
        }
    }

    let (selected_next, irq_flags) = {
        let irq = IrqGuard::new();
        if current.is_queued() {
            let _ = rq.remove(current_ptr);
            current.clear_queued();
        }
        let selected = match pick_next_task(rq, Some(current_ptr)) {
            PickResult::Switch(next) if !core::ptr::eq(current, next.as_ptr()) => Some(next),
            PickResult::Switch(_) | PickResult::KeepRunning | PickResult::GoIdle => {
                idle_thread.filter(|idle| !core::ptr::eq(current, idle.as_ptr()))
            }
        };

        if selected.is_some() {
            let flags = irq.release_keep_irqs_disabled();
            (selected, Some(flags))
        } else {
            (None, None)
        }
    };

    let Some(next) = selected_next else {
        current.set_state(TaskState::Running);
        return;
    };
    let irq_flags = irq_flags.unwrap_or(0);

    if !matches!(
        current.state(),
        TaskState::Sleeping | TaskState::Uninterruptible | TaskState::Stopped | TaskState::Dead
    ) {
        // SAFETY: restaure le flag IF capturé par IrqGuard à l'entrée de la
        // section critique ; irq_flags est la valeur RFLAGS sauvée localement, Ring 0.
        unsafe {
            IrqGuard::restore_irq_flags(irq_flags);
        }
        return;
    }

    // SAFETY: next provient de la run queue ou du TCB idle publié pour ce CPU.
    context_switch(current, &mut *next.as_ptr());
    // SAFETY: restaure le flag IF capturé par IrqGuard à l'entrée de la
    // section critique ; irq_flags est la valeur RFLAGS sauvée localement, Ring 0.
    unsafe {
        IrqGuard::restore_irq_flags(irq_flags);
    }
}

/// Enfile un TCB après réveil depuis WaitQueue.
/// À appeler depuis `wake_one`/`wake_all` pour que le thread soit reschedule.
///
/// # Safety
/// Préemption désactivée requise.
#[inline(always)]
pub unsafe fn wake_enqueue(
    rq: &mut crate::scheduler::core::runqueue::PerCpuRunQueue,
    tcb: core::ptr::NonNull<ThreadControlBlock>,
) {
    use crate::scheduler::core::task::TaskState;
    let tcb_ref = &*tcb.as_ptr();
    let should_enqueue = match tcb_ref.state() {
        TaskState::Sleeping => tcb_ref.try_transition(TaskState::Sleeping, TaskState::Runnable),
        TaskState::Uninterruptible => {
            tcb_ref.try_transition(TaskState::Uninterruptible, TaskState::Runnable)
        }
        TaskState::Stopped => tcb_ref.try_transition(TaskState::Stopped, TaskState::Runnable),
        TaskState::Runnable => true,
        TaskState::Running | TaskState::Zombie | TaskState::Dead => false,
    };
    if should_enqueue {
        rq.enqueue(tcb);
    }
}
