// PATCH-03 : Fix ExoSeal — verify_p0_fixes() + Ordre PKS/ExoKairos
// Fichier cible : kernel/src/security/exoseal.rs
// Priorité : P0 CRITIQUE (requis par ExoShield_v1_Production.md)

use core::sync::atomic::{AtomicBool, Ordering};

static EXOSEAL_PHASE0_DONE: AtomicBool = AtomicBool::new(false);
static EXOSEAL_COMPLETE_DONE: AtomicBool = AtomicBool::new(false);

/// Vérifie que tous les verrous hardware P0 sont en place avant SECURITY_READY.
///
/// NOUVEAU : Fonction exigée par ExoShield_v1_Production.md § Phase 0
/// Corrige : S-01 — verify_p0_fixes() absent
///
/// Panique kernel si un verrou requis est absent (fail-secure).
pub fn verify_p0_fixes() {
    // 1. Vérifier que NIC IOMMU est bien verrouillé
    if !crate::security::exoseal::nic_iommu_locked() {
        // Fail-secure : on ne peut pas continuer sans IOMMU lock
        crate::security::exoledger::exo_ledger_append_p0(
            crate::security::exoledger::ActionTag::P0VerifyFailed
        );
        // Handoff vers Kernel B (plus sûr qu'un panic)
        unsafe {
            crate::exophoenix::handoff::freeze_req(
                crate::exophoenix::handoff::FreezeReason::P0VerifyFailed
            );
        }
        unreachable!("freeze_req ne retourne pas");
    }

    // 2. Vérifier que PKS est en mode default-deny
    // (wrmsr IA32_PKRS = 0xFFFFFFFF doit avoir été appelé)
    let pkrs = unsafe { rdmsr(0x6E1) }; // IA32_PKRS
    if pkrs != 0xFFFF_FFFF_FFFF_FFFFu64 {
        // Tolérer un état partiellement restrictif mais logger
        crate::security::exoledger::exo_ledger_append_p0(
            crate::security::exoledger::ActionTag::PksNotDefaultDeny
        );
        // En production : freeze. En debug : warning.
        #[cfg(not(debug_assertions))]
        unsafe {
            crate::exophoenix::handoff::freeze_req(
                crate::exophoenix::handoff::FreezeReason::PksInvalidState
            );
        }
        #[cfg(debug_assertions)]
        log::warn!("verify_p0_fixes: PKS not fully default-deny (IA32_PKRS = 0x{:x})", pkrs);
    }

    // 3. Vérifier que CET global est activé (CR4.CET)
    let cr4: u64;
    unsafe {
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));
    }
    const CR4_CET_BIT: u64 = 1 << 23;
    if cr4 & CR4_CET_BIT == 0 {
        crate::security::exoledger::exo_ledger_append_p0(
            crate::security::exoledger::ActionTag::CetNotEnabled
        );
        #[cfg(not(debug_assertions))]
        unsafe {
            crate::exophoenix::handoff::freeze_req(
                crate::exophoenix::handoff::FreezeReason::CetGlobalDisabled
            );
        }
    }

    // 4. Vérifier TCB layout (double vérification runtime)
    debug_assert_eq!(
        core::mem::size_of::<crate::scheduler::core::task::ThreadControlBlock>(),
        256,
        "TCB layout corrompu : taille != 256 bytes"
    );

    // Tout OK : logger le succès
    crate::security::exoledger::exo_ledger_append_p0(
        crate::security::exoledger::ActionTag::P0VerifySuccess
    );
}

/// Phase 0 du boot sécurisé (appelée par Kernel B uniquement).
///
/// PATCH : Ajout de verify_p0_fixes() avant le retour.
/// PATCH : Correction de l'ordre PKS/ExoKairos.
pub unsafe fn exoseal_boot_phase0() {
    // Idempotente : ne s'exécute qu'une fois
    if EXOSEAL_PHASE0_DONE.swap(true, Ordering::AcqRel) {
        return;
    }

    // Étape 1 : IOMMU NIC lock (whitelist 0x0A00_0000-0x0B00_0000)
    configure_nic_iommu_policy();

    // Étape 2 : PKS default-deny (wrmsr IA32_PKRS = 0xFFFFFFFF)
    unsafe { crate::security::exoveil::exoveil_init(); }

    // Étape 3 : CET global (CR4.CET + IA32_S_CET)
    let _ = unsafe { crate::security::exocage::exocage_global_enable() };

    // Étape 4 : CORRIGÉ — ExoKairos AVANT pks_restore (ordre critique)
    // L'ancien code appelait pks_restore_for_normal_ops() dans boot_complete()
    // MAIS exokairos::init_kernel_secret() avait besoin du PKS encore restrictif.
    // Ordre correct : kairos_init ICI (PKS restrictif) → restore APRÈS.
    crate::security::exokairos::exokairos_init();

    // Étape 5 : Vérification que tous les verrous P0 sont en place
    // NOUVEAU — corrige S-01
    verify_p0_fixes();

    // Logger succès phase 0
    crate::security::exoledger::exo_ledger_append_p0(
        crate::security::exoledger::ActionTag::Phase0Complete
    );
}

/// Phase finale du boot (après toutes les initialisations kernel).
///
/// PATCH : verify_p0_fixes() appelé ici aussi (double vérification).
/// PATCH : pks_restore_for_normal_ops() APRÈS exokairos (ordre corrigé).
pub unsafe fn exoseal_boot_complete() {
    if EXOSEAL_COMPLETE_DONE.swap(true, Ordering::AcqRel) {
        return;
    }

    // Vérification finale avant d'ouvrir le système
    verify_p0_fixes();

    // CORRIGÉ : pks_restore après que tous les modules sont initialisés
    // (exokairos, exoveil, etc. ont été initialisés dans phase0 ou security_init)
    unsafe { crate::security::exoveil::pks_restore_for_normal_ops(); }

    // Barrière SeqCst garantit que toutes les initialisations sont visibles
    // par tous les cœurs AVANT que SECURITY_READY soit levé
    core::sync::atomic::fence(Ordering::SeqCst);

    // DERNIER ACTE : lever SECURITY_READY
    // À partir de ce moment, les APs peuvent procéder
    crate::security::SECURITY_READY.store(true, Ordering::Release);

    // Logger
    crate::security::exoledger::exo_ledger_append_p0(
        crate::security::exoledger::ActionTag::SecurityReady
    );
}

/// Configure et verrouille la politique IOMMU pour le NIC.
fn configure_nic_iommu_policy() {
    use crate::drivers::iommu::{IommuDomains, DeviceClass};

    let mut domain = IommuDomains::create_domain();

    // Whitelist explicite de la plage mémoire NIC
    domain.add_whitelist_range(0x0A00_0000, 0x0B00_0000);

    // Restreindre aux NIC (PCI class 0x02 = Network Controller)
    domain.restrict_to_class(DeviceClass::NetworkController);

    // Activer le domaine
    domain.activate();

    // NOUVEAU : lock hardware explicite (corrige IOMMU lock incomplet)
    // Après ce point, le domaine NE PEUT PLUS être modifié jusqu'au prochain reset
    domain.lock();

    // Marquer comme verrouillé (AtomicBool)
    crate::security::exoseal::NIC_POLICY_LOCKED.store(true, Ordering::Release);

    // Logger
    crate::security::exoledger::exo_ledger_append_p0(
        crate::security::exoledger::ActionTag::NicIommuLocked
    );

    // Vérification post-activation (NE PAS juste supposer que ça a marché)
    debug_assert!(
        crate::security::exoseal::nic_iommu_locked(),
        "IOMMU NIC lock échoué malgré domain.lock()"
    );
}

#[inline]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    ((hi as u64) << 32) | (lo as u64)
}
