// kernel/src/scheduler/fpu/save_restore.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// XSAVE/XRSTOR — Sauvegarde/Restauration FPU (Exo-OS Scheduler · Couche 1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE FPU-01 (V7-C-02 + GI-02) :
//   Ce module gère LA LOGIQUE — les instructions ASM brutes (xsave/xrstor/fxsave)
//   sont dans arch/x86_64/cpu/fpu.rs. NE PAS dupliquer les instructions ici.
//   Les appels vers arch/ sont via FFI extern "C".
//
//   MXCSR et x87 FCW sont gérés EXCLUSIVEMENT par XSAVE/XRSTOR ici.
//   switch_asm.s ne touche PAS MXCSR ni FCW (V7-C-02 — kernel compilé sans SSE).
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::Ordering;
use super::state::{FpuState, XSAVE_AREA_SIZE};
use crate::scheduler::core::task::ThreadControlBlock;

// ─────────────────────────────────────────────────────────────────────────────
// FFI vers arch/x86_64/cpu/fpu.rs (instructions ASM brutes)
// ─────────────────────────────────────────────────────────────────────────────

extern "C" {
    fn arch_xsave64(area: *mut u8, rfbm: u64);
    fn arch_xrstor64(area: *const u8, rfbm: u64);
    fn arch_fxsave64(area: *mut u8);
    fn arch_fxrstor64(area: *const u8);
    fn arch_has_xsave() -> u8;
    #[allow(dead_code)]
    fn arch_has_avx() -> u8;
}

/// Cache statique de la disponibilité XSAVE (lu une seule fois au boot).
static HAS_XSAVE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Initialise le module save_restore (détecte XSAVE, appelle detect_xsave_size).
/// Appelé depuis scheduler::init(), step 3.
pub fn init() {
    super::state::detect_xsave_size();
    // SAFETY: arch_has_xsave() lit CPUID, aucun effet de bord.
    let has = unsafe { arch_has_xsave() != 0 };
    HAS_XSAVE.store(has, Ordering::Release);
}

// ─────────────────────────────────────────────────────────────────────────────
// Sauvegarder l'état FPU du thread courant
// ─────────────────────────────────────────────────────────────────────────────

/// Sauvegarde l'état FPU du thread courant dans son `FpuState`.
///
/// Appelé depuis `switch.rs` (step 1 avant context_switch_asm) lorsque
/// `prev.fpu_loaded() == true`.
///
/// # Safety
/// - `tcb.fpu_state_ptr` doit pointer vers un `FpuState` valide, aligné 64 bytes.
/// - La FPU doit être chargée dans les registres physiques (FPU_LOADED flag).
pub unsafe fn xsave_current(tcb: &mut ThreadControlBlock) {
    let state_ptr = tcb.fpu_state_ptr as *mut FpuState;
    if state_ptr.is_null() {
        // FpuState pas encore allouée — rien à sauvegarder.
        // BUG-FIX N : marquer FPU comme non chargée même dans ce cas.
        // Sans ce store, prev.fpu_loaded reste `true` après le context switch,
        // créant une incohérence visible lors d'un éventuel accès concurrent
        // (ex. migration). Le flag est corrigé par context_switch step-4 quand
        // ce thread est reprogrammé comme `next`, mais l'expliciter ici est plus sûr.
        tcb.set_fpu_loaded(false);
        return;
    }

    let state = &mut *state_ptr;
    state.refresh_size();
    state.generation = state.generation.wrapping_add(1);

    // SAFETY: state.as_mut_ptr() est aligné 64 bytes (garanti par FpuState::repr(align(64))).
    // Le masque 0xFFFFFFFF_FFFFFFFF sauvegarde tous les composants XCR0 activés.
    if HAS_XSAVE.load(Ordering::Relaxed) {
        arch_xsave64(state.as_mut_ptr(), !0u64);
    } else {
        arch_fxsave64(state.as_mut_ptr());
    }

    // Marquer la FPU comme non chargée dans le TCB.
    tcb.set_fpu_loaded(false);
}

/// Restaure l'état FPU d'un thread dans les registres physiques.
///
/// Appelé depuis le handler #NM (Device Not Available) lorsque
/// `fpu_loaded() == false` et que le thread utilise une instruction FP.
///
/// # Safety
/// - `tcb.fpu_state_ptr` doit pointer vers un `FpuState` valide.
/// - Appelé avec préemption désactivée (pour éviter migration entre XRSTOR et FPU_LOADED).
pub unsafe fn xrstor_for(tcb: &mut ThreadControlBlock) {
    let state_ptr = tcb.fpu_state_ptr as *mut FpuState;
    if state_ptr.is_null() {
        // Ce thread n'a jamais sauvegardé de FPU → charger l'état initial.
        init_fpu_registers();
        tcb.set_fpu_loaded(true);
        return;
    }

    let state = &*state_ptr;

    // SAFETY: state.as_ptr() est aligné 64 bytes. Le masque !0u64 restaure tous les composants.
    if HAS_XSAVE.load(Ordering::Relaxed) {
        arch_xrstor64(state.as_ptr(), !0u64);
    } else {
        arch_fxrstor64(state.as_ptr());
    }

    tcb.set_fpu_loaded(true);
}

/// Initialise les registres FPU à leur état par défaut (FINIT + LDMXCSR).
unsafe fn init_fpu_registers() {
    // SAFETY: instructions FPU standard disponibles sur tout x86_64.
    core::arch::asm!(
        "fninit",
        "ldmxcsr [{mxcsr}]",
        mxcsr = in(reg) &0x1F80u32,  // Mode arrondi par défaut, masques actifs
        options(nomem, nostack),
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Allocation de FpuState pour un thread (première utilisation FPU)
// ─────────────────────────────────────────────────────────────────────────────

/// Alloue une zone FpuState pour le thread `tcb` lors de sa première
/// utilisation FPU (#NM quand fpu_state_ptr == NULL).
///
/// Retourne `true` si l'allocation a réussi.
///
/// RÈGLE : Utilise l'allocateur heap kernel (global_allocator). Ne peut PAS
/// être appelé depuis un contexte IN_RECLAIM (deadlock potentiel).
pub unsafe fn alloc_fpu_state(tcb: &mut ThreadControlBlock) -> bool {
    use core::alloc::Layout;

    // BUG-FIX F : interdire les allocations depuis un contexte IN_RECLAIM.
    // Appeler l'allocateur heap depuis IN_RECLAIM peut créer un deadlock si
    // l'allocateur lui-même attend l'EmergencyPool (RÈGLE FPU-03).
    if tcb.sched_state.load(Ordering::Relaxed)
        & crate::scheduler::core::task::SCHED_IN_RECLAIM_BIT != 0
    {
        return false;
    }

    // Layout : 2688 bytes, aligné 64 bytes.
    let layout = Layout::new::<FpuState>();

    // Utilise l'API Rust standard (alloc::alloc::alloc) qui délègue au
    // #[global_allocator] KernelAllocator — évite l'appel FFI direct à
    // __rust_alloc qui n'est pas résolu sur cible bare-metal sans usage alloc.
    let ptr = alloc::alloc::alloc(layout);
    if ptr.is_null() {
        return false;
    }

    // Initialiser la zone FpuState à l'état par défaut x87/SSE.
    let fpu = &mut *(ptr as *mut FpuState);
    core::ptr::write(fpu, FpuState::new());
    fpu.active_size = XSAVE_AREA_SIZE.load(Ordering::Relaxed);

    tcb.fpu_state_ptr = ptr as u64;
    true
}
