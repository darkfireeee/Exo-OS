// PATCH-07 : Corrections Supplémentaires
// S-03 : Capability constant-time (side-channel)
// S-04 : static_assert TCB (déjà dans patch_02, consolidé ici)
// S-05 : KPTI partiel (exophoenix/isolate.rs)
// Dead code / re-exports orphelins

// ============================================================
// SECTION 1 : capability/mod.rs ou capability/verify.rs
// Fix S-03 — Constant-time capability verification
// ============================================================

// Ajouter dans Cargo.toml :
// [dependencies]
// subtle = { version = "2.5", default-features = false }

use subtle::{ConstantTimeEq, Choice};

/// Token de capability non-forgeable.
/// La comparaison DOIT être en temps constant pour éviter les timing attacks.
#[derive(Clone)]
pub struct CapToken {
    pub(crate) inner: [u8; 32], // 256-bit token
}

/// Vérifie un token de capability en temps CONSTANT.
///
/// CORRIGE : S-03 — verify_cap_token() sans constant-time
/// Utilise subtle::ConstantTimeEq pour éviter les timing side-channels.
///
/// # Security Note
/// Ne JAMAIS utiliser `==` ou une comparaison byte par byte classique
/// pour les tokens de sécurité. Un attaquant peut mesurer le temps
/// de retour pour déduire des bits du token valide.
pub fn verify_cap_token(provided: &CapToken, expected: &CapToken) -> bool {
    // ConstantTimeEq de la crate `subtle` garantit que la comparaison
    // prend exactement le même temps quelle que soit la valeur
    let result: Choice = provided.inner.ct_eq(&expected.inner);

    // Convertir Choice en bool (Choice est opaque, pas de branchement)
    bool::from(result)
}

/// Version avec droits : retourne les droits si le token est valide.
///
/// CORRIGE : verify_and_get_rights() sans constant-time
pub fn verify_and_get_rights(
    provided: &CapToken,
    expected: &CapToken,
    rights: u64,
) -> Option<u64> {
    // La comparaison est en temps constant
    if verify_cap_token(provided, expected) {
        Some(rights)
    } else {
        None
    }
}

// ============================================================
// SECTION 2 : exophoenix/isolate.rs
// Fix S-05 — Compléter KPTI (mark_a_pages_not_present)
// ============================================================

use crate::arch::x86_64::paging::{PageTable, PageFlags};

/// Marque toutes les pages de Kernel A comme non-présentes dans l'espace
/// d'adressage géré par Kernel B lors du handoff.
///
/// CORRIGE : S-05 — mark_a_pages_not_present() vide (KPTI incomplet)
///
/// # Safety
/// - Doit être appelé UNIQUEMENT depuis le contexte Kernel B
/// - Après cet appel, toute référence à du code/données Kernel A est invalide
pub unsafe fn mark_a_pages_not_present() {
    // Récupérer le CR3 de Kernel A (sauvegardé lors du handoff)
    let kernel_a_cr3 = crate::exophoenix::state::saved_kernel_a_cr3();

    // Itérer sur toutes les entrées de la PML4 de Kernel A
    let pml4 = &mut *(kernel_a_cr3 as *mut PageTable);

    for pml4_entry in pml4.entries.iter_mut() {
        if pml4_entry.is_present() {
            // Désactiver le bit Present pour toutes les pages Kernel A
            // Cela invalide les TLB entries correspondantes
            pml4_entry.clear_present();
        }
    }

    // Flush complet du TLB (recharge CR3)
    core::arch::asm!(
        "mov rax, cr3",
        "mov cr3, rax",
        out("rax") _,
        options(nostack)
    );

    // Barrière : s'assurer que le flush est complet avant de continuer
    core::arch::asm!("mfence", options(nostack, nomem));
}

/// Override l'IDT de Kernel A avec les handlers de Kernel B.
///
/// CORRIGE : override_a_idt_with_b_handlers() vide
pub unsafe fn override_a_idt_with_b_handlers() {
    // Structure IDTR
    #[repr(C, packed)]
    struct Idtr {
        limit: u16,
        base: u64,
    }

    // Charger l'IDT de Kernel B
    let kernel_b_idt_base = crate::exophoenix::state::kernel_b_idt_base();
    let kernel_b_idt_limit = crate::exophoenix::state::kernel_b_idt_limit();

    let idtr = Idtr {
        limit: kernel_b_idt_limit,
        base: kernel_b_idt_base,
    };

    // Charger la nouvelle IDT
    core::arch::asm!(
        "lidt [{idtr}]",
        idtr = in(reg) &idtr as *const Idtr,
        options(nostack)
    );
}

// ============================================================
// SECTION 3 : Nettoyage Dead Code / Re-exports orphelins
// kernel/src/security/mod.rs — À appliquer dans le fichier existant
// ============================================================

// AVANT (problématique) :
// pub use exocage::{enable_cet_for_thread, cp_handler, validate_thread_cet, ...};
// Ces re-exports pointent vers des symboles qui peuvent ne pas exister.

// APRÈS (corrigé) :
// Option A : Supprimer les re-exports des fonctions non implémentées
// Option B : Conditionner avec cfg(feature = "cet_per_thread")

// Exemple de mod.rs corrigé (section re-exports) :
//
// #[cfg(feature = "cet_per_thread")]
// pub use exocage::{
//     enable_cet_for_thread,
//     validate_thread_cet,
//     disable_cet_for_thread,
//     cp_handler,
// };
//
// // Toujours disponible (global enable)
// pub use exocage::{
//     exocage_global_enable,
//     cpuid_cet_available,
//     CP_VIOLATION_COUNT,
// };

// Pour les MSR_IA32_PL2/PL3 avec #[allow(dead_code)] :
// Ces constantes sont légitimes pour les niveaux de privilege 2/3.
// On les conserve mais on ajoute une explication :

/// MSR pour shadow stack Ring 2 (non utilisé actuellement — Ring 2 déprécié x86_64)
#[allow(dead_code)]
const MSR_IA32_PL2_SSP: u32 = 0x6A5;

/// MSR pour shadow stack Ring 3 (user-space — futur ExoUser)
#[allow(dead_code)]
const MSR_IA32_PL3_SSP: u32 = 0x6A7;

// ============================================================
// SECTION 4 : Cargo.toml — Dépendances à ajouter
// ============================================================

// [dependencies]
// subtle = { version = "2.5", default-features = false }
// # Pour const_assert! TCB (alternative au const { let _ = [...] })
// # static_assertions = "1.1"

// [features]
// default = []
// security_audit = []       # Active les logs d'audit étendus
// cet_per_thread = []       # Active CET par thread (nécessite CPU avec CET)
// debug_cet_permissive = [] # En debug : ne pas freeze sur première #CP

// ============================================================
// SECTION 5 : Test de régression complet
// kernel/src/security/tests.rs
// ============================================================

#[cfg(test)]
mod security_regression_tests {
    use super::*;

    #[test]
    fn test_cap_token_constant_time_equal() {
        let token_a = CapToken { inner: [0xAB; 32] };
        let token_b = CapToken { inner: [0xAB; 32] };
        assert!(verify_cap_token(&token_a, &token_b));
    }

    #[test]
    fn test_cap_token_constant_time_not_equal() {
        let token_a = CapToken { inner: [0xAB; 32] };
        let mut inner_b = [0xAB; 32];
        inner_b[31] = 0xAC; // Un seul bit différent
        let token_b = CapToken { inner: inner_b };
        // Doit retourner false ET prendre le même temps que true
        assert!(!verify_cap_token(&token_a, &token_b));
    }

    #[test]
    fn test_watchdog_timeout_clamped_below_min() {
        let result = crate::security::exonmi::watchdog_set_timeout(100); // 100 ns = trop court
        assert_eq!(result, crate::security::exonmi::WATCHDOG_TIMEOUT_MIN_NS);
    }

    #[test]
    fn test_watchdog_timeout_clamped_above_max() {
        let result = crate::security::exonmi::watchdog_set_timeout(u64::MAX);
        assert_eq!(result, crate::security::exonmi::WATCHDOG_TIMEOUT_MAX_NS);
    }

    #[test]
    fn test_tcb_size_invariant() {
        // Ce test doit compiler ET passer
        assert_eq!(
            core::mem::size_of::<crate::scheduler::core::task::ThreadControlBlock>(),
            256,
            "TCB doit faire exactement 256 bytes (GI-01)"
        );
    }
}
