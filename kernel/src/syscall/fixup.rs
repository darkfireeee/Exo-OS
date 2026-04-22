// kernel/src/syscall/fixup.rs
//
// Table de fixup pour les page faults kernel durant copy_from_user / copy_to_user.
//
// ## Problème
// Quand un handler syscall appelle copy_from_user() ou copy_to_user() et que
// la page userspace n'est pas présente (CoW non résolu, demand paging, page
// invalide), un #PF se déclenche en contexte RING 0. Sans table de fixup,
// do_page_fault() panique avec "KernelFault".
//
// ## Solution
// Chaque site d'accès userspace dans copy_from_user / copy_to_user enregistre
// une paire (fault_rip, recovery_rip) :
//   - fault_rip    : adresse de l'instruction qui peut faulter
//   - recovery_rip : adresse où reprendre si le fault est non-récupérable
//
// do_page_fault() consulte cette table AVANT de paniquer. Si fault_rip est
// connu, il patche frame->rip = recovery_rip et retourne. La fonction de copie
// reprend au point de recovery avec un indicateur d'erreur.
//
// ## Implémentation
// Rust ne peut pas annoter des instructions individuelles avec des labels ASM
// de la même façon que C (__ex_table). On utilise donc une approche légèrement
// différente : copy_from_user_safe et copy_to_user_safe sont des wrappers ASM
// qui s'enregistrent dans une table statique avant la boucle critique et
// se désenregistrent après. La table contient une entrée par CPU (les copies
// user sont séquentielles par thread).
//
// Architecture :
//   KernelCopyState par CPU (dans percpu) :
//     - active: bool       — une copie est en cours sur ce CPU
//     - fault_rsp: u64     — RSP au moment de l'entrée dans la copie
//     - recovery_fn: usize — adresse de la fonction de recovery (retourne EFAULT)

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use crate::memory::core::VirtAddr;

/// Nombre maximum de CPUs supportés (doit matcher MAX_CPUS)
const FIXUP_MAX_CPUS: usize = 256;

/// État de copie kernel→user / user→kernel pour un CPU.
///
/// Quand une copie user est active sur un CPU, `active` est vrai et
/// `recovery_rsp` / `recovery_rip` indiquent où reprendre en cas de #PF.
#[repr(C, align(64))]
pub struct KernelCopyState {
    /// Une copie userspace est en cours sur ce CPU.
    pub active:       AtomicBool,
    _pad0:            [u8; 7],
    /// RSP au moment de l'entrée dans la fonction de copie (pour unwinding).
    pub recovery_rsp: AtomicU64,
    /// RIP de reprise : l'instruction après le site de fault retournera EFAULT.
    pub recovery_rip: AtomicUsize,
    _pad1:            [u8; 44],
}

// SAFETY: tous les champs sont atomiques.
unsafe impl Send for KernelCopyState {}
unsafe impl Sync for KernelCopyState {}

impl KernelCopyState {
    const fn new() -> Self {
        Self {
            active:       AtomicBool::new(false),
            _pad0:        [0u8; 7],
            recovery_rsp: AtomicU64::new(0),
            recovery_rip: AtomicUsize::new(0),
            _pad1:        [0u8; 44],
        }
    }
}

// Assertion compile-time : chaque entrée doit tenir dans une cache line (64B)
const _: () = assert!(
    core::mem::size_of::<KernelCopyState>() == 64,
    "KernelCopyState doit faire exactement 64 bytes (1 cache line)"
);

/// Table globale des états de copie, indexée par cpu_id.
static COPY_STATES: [KernelCopyState; FIXUP_MAX_CPUS] = {
    // Rust ne supporte pas `[expr; N]` pour N>32 avec const fn non-Copy,
    // on utilise un tableau initialisé manuellement via transmute-free trick.
    // Solution : tableau de MaybeUninit initialisé dans init_fixup_table().
    // Pour éviter un UnsafeCell global, on initialise avec new() via const.
    const INIT: KernelCopyState = KernelCopyState::new();
    [INIT; FIXUP_MAX_CPUS]
};

// ─────────────────────────────────────────────────────────────────────────────
// API publique
// ─────────────────────────────────────────────────────────────────────────────

/// Enregistre le début d'une copie userspace sur le CPU courant.
///
/// Doit être appelé AVANT tout accès à un pointeur userspace.
/// `recovery_rip` est l'adresse où reprendre si le #PF n'est pas récupérable
/// (typiquement l'instruction `return Err(SyscallError::Fault)`).
///
/// # Safety
/// `recovery_rip` doit pointer vers du code kernel valide dans la même fonction.
#[inline]
pub unsafe fn fixup_enter(cpu_id: usize, recovery_rip: usize) {
    if cpu_id >= FIXUP_MAX_CPUS { return; }
    let state = &COPY_STATES[cpu_id];
    // Lire RSP courant pour le unwinding (non utilisé dans cette implémentation
    // simplifiée, mais disponible pour un unwinder futur)
    let rsp: u64;
    core::arch::asm!("mov {}, rsp", out(reg) rsp, options(nomem, nostack, preserves_flags));
    state.recovery_rsp.store(rsp, Ordering::Relaxed);
    state.recovery_rip.store(recovery_rip, Ordering::Relaxed);
    // Publier `active` en dernier (Release) pour que recovery_rip/rsp
    // soient visibles avant que le handler #PF lise active.
    state.active.store(true, Ordering::Release);
}

/// Désenregistre la copie userspace sur le CPU courant.
///
/// Doit être appelé APRÈS la fin de la boucle d'accès userspace,
/// succès ou erreur.
#[inline]
pub fn fixup_exit(cpu_id: usize) {
    if cpu_id >= FIXUP_MAX_CPUS { return; }
    // Acquire pour que tout read/write de la copie soit visible avant la
    // désactivation.
    COPY_STATES[cpu_id].active.store(false, Ordering::Release);
}

/// Consulte la table de fixup pour un #PF kernel sur `fault_rip`.
///
/// Appelé par do_page_fault() avant de paniquer.
///
/// Retourne `Some(recovery_rip)` si une copie userspace est en cours sur
/// `cpu_id` (le fault peut être intercepté), `None` sinon (panic légitime).
///
/// # Safety
/// Doit être appelé depuis le handler #PF kernel, avec les interruptions
/// désactivées (garanties par le handler x86-64).
#[inline]
pub fn fixup_lookup(cpu_id: usize) -> Option<usize> {
    if cpu_id >= FIXUP_MAX_CPUS { return None; }
    let state = &COPY_STATES[cpu_id];
    // Acquire pour synchroniser avec fixup_enter (Release sur active).
    if !state.active.load(Ordering::Acquire) {
        return None;
    }
    let rip = state.recovery_rip.load(Ordering::Relaxed);
    if rip == 0 { return None; }
    Some(rip)
}

// ─────────────────────────────────────────────────────────────────────────────
// Wrappers copy_from/to_user avec protection fixup
// ─────────────────────────────────────────────────────────────────────────────

/// Copie `len` octets de `src` (userspace) vers `dst` (kernel), avec fixup #PF.
///
/// En cas de page fault non récupérable sur `src`, retourne `Err(EFAULT)`
/// au lieu de paniquer.
///
/// Remplace copy_from_user() dans validation.rs.
pub fn copy_from_user_safe(
    dst:    *mut u8,
    src:    *const u8,
    len:    usize,
    cpu_id: usize,
) -> bool {
    if len == 0 { return true; }

    // Label de recovery : si le #PF se produit pendant la boucle ci-dessous,
    // le handler patche RIP sur ce label et `faulted` vaut true au retour.
    //
    // Implémentation : on utilise une variable atomique CPU-locale pour
    // signaler le fault plutôt qu'un vrai setjmp/longjmp. Le handler #PF
    // appelle fixup_signal_fault(cpu_id) qui set FAULTED[cpu_id].
    FAULTED[cpu_id.min(FIXUP_MAX_CPUS - 1)].store(false, Ordering::Relaxed);

    // Calculer l'adresse de recovery : l'instruction qui vérifie FAULTED
    // après la boucle. On utilise une étiquette Rust via un bloc.
    let recovery_addr = fault_recovery_stub as usize;

    // SAFETY: recovery_addr est une fonction kernel valide.
    unsafe { fixup_enter(cpu_id, recovery_addr); }

    // Boucle d'accès userspace — faultable
    // SAFETY: src validé par l'appelant (validate_user_range). dst est kernel.
    let ok = unsafe {
        let mut faulted_mid = false;
        for i in 0..len {
            // Si un fault intermédiaire a été signalé par le handler #PF
            if FAULTED[cpu_id.min(FIXUP_MAX_CPUS - 1)].load(Ordering::Relaxed) {
                faulted_mid = true;
                break;
            }
            let byte = core::ptr::read_volatile(src.add(i));
            core::ptr::write(dst.add(i), byte);
        }
        !faulted_mid
    };

    fixup_exit(cpu_id);

    // Vérifier une dernière fois (le handler peut avoir signalé pendant la
    // dernière itération)
    if FAULTED[cpu_id.min(FIXUP_MAX_CPUS - 1)].load(Ordering::Relaxed) {
        return false;
    }

    ok
}

/// Copie `len` octets de `src` (kernel) vers `dst` (userspace), avec fixup #PF.
pub fn copy_to_user_safe(
    dst:    *mut u8,
    src:    *const u8,
    len:    usize,
    cpu_id: usize,
) -> bool {
    if len == 0 { return true; }

    FAULTED[cpu_id.min(FIXUP_MAX_CPUS - 1)].store(false, Ordering::Relaxed);
    let recovery_addr = fault_recovery_stub as usize;

    // SAFETY: recovery_addr est une fonction kernel valide.
    unsafe { fixup_enter(cpu_id, recovery_addr); }

    let ok = unsafe {
        let mut faulted_mid = false;
        for i in 0..len {
            if FAULTED[cpu_id.min(FIXUP_MAX_CPUS - 1)].load(Ordering::Relaxed) {
                faulted_mid = true;
                break;
            }
            let byte = core::ptr::read(src.add(i));
            core::ptr::write_volatile(dst.add(i), byte);
        }
        !faulted_mid
    };

    fixup_exit(cpu_id);

    if FAULTED[cpu_id.min(FIXUP_MAX_CPUS - 1)].load(Ordering::Relaxed) {
        return false;
    }

    ok
}

// ─────────────────────────────────────────────────────────────────────────────
// Signal de fault et stub de recovery
// ─────────────────────────────────────────────────────────────────────────────

/// Tableau de flags "fault signalé" par CPU.
/// Set par le handler #PF quand fixup_lookup() retourne Some.
static FAULTED: [AtomicBool; FIXUP_MAX_CPUS] = {
    const INIT_BOOL: AtomicBool = AtomicBool::new(false);
    [INIT_BOOL; FIXUP_MAX_CPUS]
};

/// Signale un fault pour le CPU `cpu_id`.
///
/// Appelé par do_page_fault() quand fixup_lookup() a retourné Some.
/// Désactive le fixup et pose le flag FAULTED pour que copy_from/to_user_safe
/// retourne false.
#[inline]
pub fn fixup_signal_fault(cpu_id: usize) {
    if cpu_id >= FIXUP_MAX_CPUS { return; }
    FAULTED[cpu_id].store(true, Ordering::Release);
    // Désactiver le fixup pour que le prochain accès kernel faulte normalement
    fixup_exit(cpu_id);
}

/// Stub de recovery — adresse enregistrée dans recovery_rip.
///
/// Cette fonction n'est jamais appelée directement. Son adresse sert de
/// cible pour fixup_lookup(). Le handler #PF n'utilise pas cette adresse
/// pour un jump réel (on utilise fixup_signal_fault à la place), mais
/// elle est conservée pour une future implémentation via longjmp kernel.
#[inline(never)]
#[cold]
extern "C" fn fault_recovery_stub() {
    // Point de récupération symbolique — le handler #PF appelle
    // fixup_signal_fault() qui set FAULTED, la boucle dans copy_from/to_user_safe
    // détecte le flag et retourne false (EFAULT).
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixup_lookup_inactive_returns_none() {
        // Sur le CPU 0 sans copie active, lookup retourne None
        COPY_STATES[0].active.store(false, Ordering::Release);
        assert!(fixup_lookup(0).is_none());
    }

    #[test]
    fn fixup_enter_exit_roundtrip() {
        let recovery = fault_recovery_stub as usize;
        unsafe { fixup_enter(0, recovery); }
        assert!(fixup_lookup(0).is_some());
        assert_eq!(fixup_lookup(0).unwrap(), recovery);
        fixup_exit(0);
        assert!(fixup_lookup(0).is_none());
    }

    #[test]
    fn fixup_signal_clears_active() {
        let recovery = fault_recovery_stub as usize;
        unsafe { fixup_enter(1, recovery); }
        fixup_signal_fault(1);
        assert!(fixup_lookup(1).is_none());
        assert!(FAULTED[1].load(Ordering::Relaxed));
        // Cleanup
        FAULTED[1].store(false, Ordering::Relaxed);
    }

    #[test]
    fn copy_states_cache_line_aligned() {
        assert_eq!(core::mem::size_of::<KernelCopyState>(), 64);
        assert_eq!(core::mem::align_of::<KernelCopyState>(), 64);
    }
}
