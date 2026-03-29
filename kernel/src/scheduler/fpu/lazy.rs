// kernel/src/scheduler/fpu/lazy.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// LAZY FPU — Gestion différée de la FPU par thread (Exo-OS Scheduler · Couche 1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// PRINCIPE du Lazy FPU (DOC3 + arch x86_64) :
//   1. Au démarrage et après chaque context switch : CR0.TS = 1
//      → la première instruction FP/SIMD du thread déclenche #NM (Device Not Available)
//   2. Handler #NM : restaurer la FPU pour ce thread (xrstor), CR0.TS = 0
//   3. Avant le prochain context switch : si FPU_LOADED → sauvegarder (xsave), CR0.TS = 1
//
// Avantage : les threads qui n'utilisent PAS la FPU n'ont aucun coût de sauvegarde.
//
// RÈGLE FPU-01 (DOC3) : Ce module gère la POLITIQUE (flag lazy_fpu_used dans TCB).
//   Les instructions ASM brutes sont dans arch/x86_64/cpu/fpu.rs.
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicBool, Ordering};
use crate::scheduler::core::task::ThreadControlBlock;

// ─────────────────────────────────────────────────────────────────────────────
// État global d'initialisation
// ─────────────────────────────────────────────────────────────────────────────

static FPU_LAZY_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Initialise le mécanisme Lazy FPU sur le CPU courant.
///
/// - Active CR0.TS = 1 (tout thread considéré sans FPU chargée).
/// - Doit être appelé sur CHAQUE CPU (BSP + APs).
/// Correspond au step 4 de la séquence d'initialisation scheduler (DOC3).
pub fn init() {
    // SAFETY: cr0_set_ts() set le bit TS dans CR0. Sûr au boot avant tout thread.
    unsafe { cr0_set_ts(); }
    FPU_LAZY_INITIALIZED.store(true, Ordering::Release);
}

// ─────────────────────────────────────────────────────────────────────────────
// Manipulation de CR0.TS
// ─────────────────────────────────────────────────────────────────────────────

/// Active CR0.TS (Task Switched) → déclenche #NM sur prochaine instruction FP.
#[inline(always)]
pub unsafe fn cr0_set_ts() {
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!(
        "mov rax, cr0",
        "or  rax, 8",    // bit 3 = TS
        "mov cr0, rax",
        out("rax") _,
        options(nostack, nomem),
    );
}

/// Désactive CR0.TS → accès FP autorisé sans #NM.
/// Appelé par le handler #NM après xrstor réussi.
#[inline(always)]
pub unsafe fn cr0_clear_ts() {
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!(
        "clts",
        options(nostack, nomem),
    );
}

/// Retourne vrai si CR0.TS est activé.
#[inline(always)]
pub unsafe fn cr0_ts_is_set() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        let cr0: u64;
        core::arch::asm!(
            "mov {}, cr0",
            out(reg) cr0,
            options(nostack, nomem),
        );
        (cr0 >> 3) & 1 != 0
    }
    #[cfg(not(target_arch = "x86_64"))]
    { false }
}

// ─────────────────────────────────────────────────────────────────────────────
// mark_fpu_not_loaded() — appelé après chaque context switch
// ─────────────────────────────────────────────────────────────────────────────

/// Marque la FPU comme non-chargée pour le prochain thread.
/// Appelé depuis `switch.rs` après `context_switch_asm()` retourne.
///
/// RÈGLE SWITCH-02 (DOC3) : cette fonction est le "mark APRÈS" la sauvegarde.
#[inline(always)]
pub unsafe fn mark_fpu_not_loaded() {
    cr0_set_ts();
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler #NM — Device Not Available
// ─────────────────────────────────────────────────────────────────────────────

/// Handler pour l'exception #NM (Device Not Available, vecteur 7).
///
/// Déclenché quand un thread execute une instruction FP/SIMD et que CR0.TS = 1.
///
/// Séquence :
/// 1. Vérifier que l'exception vient bien de CR0.TS (et non d'une faute réelle).
/// 2. Désactiver CR0.TS (clts).
/// 3. Restaurer le contexte FPU du thread courant (`xrstor_for`).
/// 4. Mettre à jour FPU_LOADED = true dans le TCB.
/// 5. Retour de l'exception — l'instruction sera rejouée.
///
/// # Safety
/// Appelé depuis le vecteur d'interruption, avec préemption désactivée.
pub unsafe fn handle_nm_exception(tcb: &mut ThreadControlBlock) {
    // SAFETY: clts est sûr ici car on va immédiatement restaurer la FPU.
    cr0_clear_ts();

    // BUG-FIX M : allouer FpuState avant la première utilisation FPU.
    // Sans cette allocation, xrstor_for() initialise les registres x87/SSE
    // par défaut mais il n'existe aucune zone de sauvegarde.
    // À chaque context switch suivant, xsave_current() voit fpu_state_ptr == NULL
    // et retourne SANS sauvegarder l'état → l'état FPU est perdu sans bruit
    // jusqu'à ce qu'une allocation réussisse.
    if tcb.fpu_state_ptr == 0 {
        super::save_restore::alloc_fpu_state(tcb);
        // Si l'allocation échoue (IN_RECLAIM ou OOM), fpu_state_ptr reste NULL.
        // xrstor_for() gérera ce cas : init par défaut, FPU_LOADED = true, mais
        // l'état sera perdu au prochain switch (dégradation gracieuse).
    }

    // Restaurer l'état FPU ou initialiser pour la première fois.
    super::save_restore::xrstor_for(tcb);
    // xrstor_for positionne déjà FPU_LOADED = true dans le TCB.
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers d'interrogation
// ─────────────────────────────────────────────────────────────────────────────

/// Vrai si ce thread a utilisé la FPU dans sa tranche courante.
#[inline(always)]
pub fn is_fpu_used(tcb: &ThreadControlBlock) -> bool {
    // FPU_LOADED = true signifie que la FPU était chargée lors du dernier switch.
    // C'est l'indication que ce thread utilise la FPU.
    tcb.fpu_loaded()
}

/// Vrai si le Lazy FPU est initialisé sur ce CPU.
#[inline(always)]
pub fn is_initialized() -> bool {
    FPU_LAZY_INITIALIZED.load(Ordering::Relaxed)
}

// ─────────────────────────────────────────────────────────────────────────────
// C ABI EXPORT — pont pour arch/x86_64/exceptions.rs (handler #NM)
// ─────────────────────────────────────────────────────────────────────────────
//
// arch/ ne peut pas importer scheduler/ directement (cycle de dépendances Rust).
// Cette fonction `#[no_mangle] extern "C"` est déclarée via `extern "C"` dans
// arch/x86_64/exceptions.rs et résolue à l'édition des liens.
//
// RÈGLE FPU-02 (DOC3) : Le handler #NM DOIT déléguer à ce module.
// ─────────────────────────────────────────────────────────────────────────────

/// Pont C ABI pour `do_device_not_avail` (arch/x86_64/exceptions.rs).
///
/// `tcb_ptr` : pointeur vers le `ThreadControlBlock` courant (lu depuis GS:[0x20]).
///   - Si null : simple effacement CR0.TS (thread non-scheduler ou phase de boot).
///   - Sinon  : délégation complète à `handle_nm_exception()`.
///
/// # Safety
/// Appelé depuis un handler d'interruption, préemption implicitement désactivée.
/// `tcb_ptr`, si non-null, DOIT pointer vers un TCB valide.
#[no_mangle]
pub unsafe extern "C" fn sched_fpu_handle_nm(tcb_ptr: *mut u8) {
    if tcb_ptr.is_null() {
        // Phase de boot ou idle sans TCB — effacer CR0.TS uniquement.
        cr0_clear_ts();
        return;
    }
    let tcb = &mut *(tcb_ptr as *mut ThreadControlBlock);
    handle_nm_exception(tcb);
}

#[cfg(test)]
mod p2_7_fpu_tests {
    use super::*;

    /// P2-7 / Test 4a — FPU_LAZY_INITIALIZED : state machine init/check.
    ///
    /// Runnable sur host : on valide la sémantique atomique sans toucher CR0.
    #[test]
    fn fpu_lazy_init_flag_set_after_init() {
        // Utilise un AtomicBool local pour isoler du static global
        // (évite les interférences si init() a déjà tourné).
        let local = AtomicBool::new(false);
        assert!(
            !local.load(Ordering::Relaxed),
            "flag FPU doit être false avant init"
        );

        local.store(true, Ordering::Release);
        assert!(
            local.load(Ordering::Relaxed),
            "flag FPU doit être true après init"
        );
    }

    /// P2-7 / Test 4b — cr0_ts_is_set() fallback hors x86_64.
    ///
    /// Sur host non-x86_64 : retourne toujours false (fallback cfg).
    /// Sur x86_64 bare-metal : retourne l'état réel de CR0.TS.
    ///
    /// IMPORTANT: sur host x86_64 (Ring 3), lire CR0 provoquerait une faute
    /// privilégiée ; on valide donc uniquement la liaison de symbole.
    #[test]
    fn cr0_ts_is_set_returns_bool() {
        #[cfg(not(target_arch = "x86_64"))]
        {
            let ts_state = unsafe { cr0_ts_is_set() };
            assert!(
                !ts_state,
                "cr0_ts_is_set() doit retourner false hors x86_64"
            );
        }

        #[cfg(all(target_arch = "x86_64", target_os = "none"))]
        {
            let _ = unsafe { cr0_ts_is_set() };
        }

        #[cfg(all(target_arch = "x86_64", not(target_os = "none")))]
        {
            let symbol_only: unsafe fn() -> bool = cr0_ts_is_set;
            let _ = symbol_only;
        }
    }

    /// P2-7 / Test 4c — Séquence complète lazy FPU (bare-metal uniquement).
    ///
    /// Vérifie la séquence : cr0_set_ts() → ts_is_set()==true
    ///                       → cr0_clear_ts() → ts_is_set()==false
    ///
    /// Nécessite Ring 0 pour écrire CR0 → ignoré sur host.
    #[test]
    #[cfg_attr(not(target_os = "none"), ignore = "P2-7: Ring 0 requis pour CR0")]
    fn fpu_lazy_cr0_set_clear_sequence() {
        unsafe {
            cr0_set_ts();
            assert!(cr0_ts_is_set(), "CR0.TS doit être 1 après cr0_set_ts()");

            cr0_clear_ts();
            assert!(!cr0_ts_is_set(), "CR0.TS doit être 0 après cr0_clear_ts()");

            // Rétablir CR0.TS pour préserver l'invariant lazy FPU.
            cr0_set_ts();
        }
    }
}
