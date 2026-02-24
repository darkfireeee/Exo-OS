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
