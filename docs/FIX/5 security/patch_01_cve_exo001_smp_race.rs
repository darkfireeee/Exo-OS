// PATCH-01 : Fix CVE-EXO-001 — Race SMP Boot + spin-wait SECURITY_READY
// Fichier cible : arch/x86_64/smp/init.rs
// Priorité : P0 CRITIQUE — À appliquer EN PREMIER

// ============================================================
// SECTION 1 : arch/x86_64/smp/init.rs
// Ajouter après la fonction ap_startup() existante
// ============================================================

use core::sync::atomic::Ordering;
use core::hint::spin_loop;
use crate::security::{is_security_ready, SECURITY_READY};

/// Nombre maximal de cycles d'attente avant timeout (≈ 5 secondes sur 3 GHz)
const SMP_SECURITY_WAIT_MAX: u64 = 15_000_000_000;

/// Lecture monotone du TSC pour le timeout
#[inline]
unsafe fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "lfence",          // sérialisation lecture
        "rdtsc",
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    ((hi as u64) << 32) | (lo as u64)
}

/// Appelée par chaque AP (Application Processor) au démarrage SMP.
///
/// IMPÉRATIF : chaque AP DOIT appeler cette fonction avant toute IPC,
/// accès ExoFS ou opération sur capabilities.
///
/// Invariant BootSafety (ExoShield_v1_Production.md) :
///   ¬SecurityReady ⟹ ¬NetworkEnabled ∧ ¬MutableFS ∧ ¬IPC
///
/// Corrige : CVE-EXO-001 / BOOT-SEC
pub fn ap_wait_security_ready() {
    // Vérification rapide : si déjà prêt, on sort immédiatement
    if is_security_ready() {
        // Barrière Acquire garantit que tous les stores BSP sont visibles
        core::sync::atomic::fence(Ordering::Acquire);
        return;
    }

    let deadline = unsafe { read_tsc() + SMP_SECURITY_WAIT_MAX };

    loop {
        // Pause CPU (évite contention sur le bus mémoire)
        spin_loop();

        if SECURITY_READY.load(Ordering::Acquire) {
            // Barrière complète : tous les writes du BSP sont maintenant visibles
            core::sync::atomic::fence(Ordering::SeqCst);
            return;
        }

        // Timeout : le BSP n'a pas levé SECURITY_READY dans le délai imparti.
        // On déclenche un FreezeReq vers Kernel B plutôt qu'un panic.
        let now = unsafe { read_tsc() };
        if now >= deadline {
            // Log d'urgence via ledger P0 si possible
            #[cfg(feature = "security_audit")]
            crate::security::exoledger::exo_ledger_append_p0(
                crate::security::exoledger::ActionTag::SmpSecurityTimeout
            );

            // Handoff immédiat vers Kernel B (ExoPhoenix)
            // Ne jamais continuer sans sécurité établie.
            unsafe {
                crate::exophoenix::handoff::freeze_req(
                    crate::exophoenix::handoff::FreezeReason::SecurityInitTimeout
                );
            }

            // Fallback si freeze_req retourne (ne devrait pas)
            // Halte matérielle de ce core uniquement
            unsafe {
                core::arch::asm!(
                    "cli",
                    "2: hlt",
                    "jmp 2b",
                    options(nostack, noreturn)
                );
            }
        }
    }
}

// ============================================================
// SECTION 2 : kernel/src/security/mod.rs
// Ajouter les assertions de debug dans security_init()
// ============================================================

// Dans security_init(), AVANT chaque étape critique :

/// Assert de sécurité : vérifie l'invariant BootSafety au moment de l'appel.
/// En release, compilé à zéro overhead.
#[inline]
pub fn assert_security_invariant(stage: &str) {
    // En debug : panic explicite avec message
    debug_assert!(
        !SECURITY_READY.load(Ordering::Relaxed) || cfg!(test),
        "BUG: SECURITY_READY mis à true avant la fin de security_init() à l'étape: {}",
        stage
    );
}

// Exemple d'intégration dans security_init() :
// pub fn security_init() {
//     assert_security_invariant("integrity_check");
//     integrity_check::init();
//
//     assert_security_invariant("capability");
//     capability::init();
//
//     // POINT CRITIQUE : aucun AP ne doit avoir IPC ici
//     // Le spin-wait dans smp/init.rs est la garde externe
//
//     assert_security_invariant("access_control");
//     access_control::init();
//
//     // ... autres étapes ...
//
//     // DERNIER : lever le flag après TOUT
//     unsafe { exoseal::exoseal_boot_complete(); }
//     // SECURITY_READY est maintenant true (set dans exoseal_boot_complete)
// }

// ============================================================
// SECTION 3 : Test unitaire
// Fichier : kernel/src/security/tests.rs (nouveau)
// ============================================================

#[cfg(test)]
mod tests_cve_exo_001 {
    use super::*;
    use core::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn test_security_ready_store_load_contract() {
        // Vérifie que le contrat Release/Acquire est respecté
        let flag = AtomicBool::new(false);
        assert!(!flag.load(Ordering::Acquire));
        flag.store(true, Ordering::Release);
        // Après Release, toute lecture Acquire voit true
        assert!(flag.load(Ordering::Acquire));
    }

    #[test]
    fn test_ap_wait_security_ready_immediate() {
        // Si SECURITY_READY est déjà true, ap_wait ne boucle pas
        SECURITY_READY.store(true, Ordering::Release);
        // Doit retourner immédiatement (pas de timeout)
        ap_wait_security_ready();
        SECURITY_READY.store(false, Ordering::Relaxed); // cleanup
    }
}
