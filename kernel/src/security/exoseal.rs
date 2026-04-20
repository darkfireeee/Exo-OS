//! ExoSeal — boot inversé ExoShield v1.0.
//!
//! Ce module rassemble le minimum opérationnel demandé par la spec :
//! PKS default-deny, CET global, watchdog durci et verrouillage IOMMU du NIC.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::exophoenix::{ssr, stage0};
use crate::memory::dma::iommu::domain::PciBdf;
use crate::memory::dma::iommu::{DomainType, IOMMU_DOMAINS};

use super::{exocage, exoledger, exoveil, SECURITY_READY};

const NORMAL_HANDOFF_FLAG: u64 = 0;
const BOOT_PHASE0_WATCHDOG_MS: u64 = 500;
const OPERATIONAL_WATCHDOG_MS: u64 = 50;
const NIC_DMA_WHITELIST_BASE: u64 = 0x0A00_0000;
const NIC_DMA_WHITELIST_END_EXCLUSIVE: u64 = 0x0B00_0000;

static EXOSEAL_PHASE0_DONE: AtomicBool = AtomicBool::new(false);
static EXOSEAL_COMPLETE_DONE: AtomicBool = AtomicBool::new(false);
static NIC_POLICY_LOCKED: AtomicBool = AtomicBool::new(false);
static NIC_DOMAIN_ID: AtomicU32 = AtomicU32::new(0);
static NIC_DMA_BASE: AtomicU64 = AtomicU64::new(0);
static NIC_DMA_END: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum P0VerifyError {
    NicIommuUnlocked,
    CetGlobalDisabled,
    DefaultDomainRevoked,
    CapsDomainExposed,
    CredentialsDomainExposed,
    TcbHotDomainExposed,
}

fn validate_phase0_state(
    nic_locked: bool,
    cet_supported: bool,
    cet_enabled: bool,
    pks_supported: bool,
    default_revoked: bool,
    caps_revoked: bool,
    credentials_revoked: bool,
    tcb_hot_revoked: bool,
) -> Result<(), P0VerifyError> {
    if !nic_locked {
        return Err(P0VerifyError::NicIommuUnlocked);
    }
    if cet_supported && !cet_enabled {
        return Err(P0VerifyError::CetGlobalDisabled);
    }
    if pks_supported && default_revoked {
        return Err(P0VerifyError::DefaultDomainRevoked);
    }
    if pks_supported && !caps_revoked {
        return Err(P0VerifyError::CapsDomainExposed);
    }
    if pks_supported && !credentials_revoked {
        return Err(P0VerifyError::CredentialsDomainExposed);
    }
    if pks_supported && !tcb_hot_revoked {
        return Err(P0VerifyError::TcbHotDomainExposed);
    }
    Ok(())
}

pub fn verify_p0_fixes() -> Result<(), P0VerifyError> {
    let (cet_supported, _) = exocage::cpuid_cet_available();
    let result = validate_phase0_state(
        nic_iommu_locked(),
        cet_supported,
        exocage::is_cet_global_enabled(),
        exoveil::pks_available(),
        exoveil::is_domain_revoked(exoveil::PksDomain::Default),
        exoveil::is_domain_revoked(exoveil::PksDomain::Caps),
        exoveil::is_domain_revoked(exoveil::PksDomain::Credentials),
        exoveil::is_domain_revoked(exoveil::PksDomain::TcbHot),
    );

    if let Err(error) = result {
        let step = match error {
            P0VerifyError::NicIommuUnlocked => 0,
            P0VerifyError::CetGlobalDisabled => 1,
            P0VerifyError::DefaultDomainRevoked => 2,
            P0VerifyError::CapsDomainExposed => 3,
            P0VerifyError::CredentialsDomainExposed => 4,
            P0VerifyError::TcbHotDomainExposed => 5,
        };
        exoledger::exo_ledger_append_p0(exoledger::ActionTag::BootSealViolation { step });
        unsafe {
            ssr::ssr_atomic(ssr::SSR_HANDOFF_FLAG).store(1, Ordering::Release);
        }
        return Err(error);
    }

    Ok(())
}

pub fn configure_nic_iommu_policy() {
    if NIC_POLICY_LOCKED.load(Ordering::Acquire) {
        return;
    }

    if IOMMU_DOMAINS.domain_count() == 0 {
        IOMMU_DOMAINS.init();
    }

    let Ok(domain_id) = IOMMU_DOMAINS.create_domain(
        DomainType::Translated,
        NIC_DMA_WHITELIST_BASE,
        NIC_DMA_WHITELIST_END_EXCLUSIVE,
    ) else {
        return;
    };

    let mut nic_found = false;
    let _ = IOMMU_DOMAINS.with_domain_mut(domain_id, |domain| {
        for index in 0..stage0::b_device_count() {
            let Some(device) = stage0::b_device(index) else {
                continue;
            };
            if device.class_code != 0x02 {
                continue;
            }

            let _ = domain.attach_device(PciBdf::new(device.bus, device.device, device.function));
            nic_found = true;
        }

        if nic_found {
            domain.activate();
        }
    });

    if !nic_found {
        let _ = IOMMU_DOMAINS.destroy_domain(domain_id);
        return;
    }

    NIC_DOMAIN_ID.store(domain_id.0, Ordering::Release);
    NIC_DMA_BASE.store(NIC_DMA_WHITELIST_BASE, Ordering::Release);
    NIC_DMA_END.store(NIC_DMA_WHITELIST_END_EXCLUSIVE, Ordering::Release);
    NIC_POLICY_LOCKED.store(true, Ordering::Release);
    exoledger::exo_ledger_append_p0(exoledger::ActionTag::NicIommuLocked);
}

pub unsafe fn exoseal_boot_phase0() {
    if EXOSEAL_PHASE0_DONE.swap(true, Ordering::AcqRel) {
        return;
    }

    configure_nic_iommu_policy();
    // SAFETY: ExoSeal phase 0 s'exécute au boot en ring 0, avant usage normal
    // des domaines PKS.
    unsafe { exoveil::exoveil_init(); }
    // SAFETY: l'activation CET globale est un prérequis ring 0 du boot ExoShield.
    let _ = unsafe { exocage::exocage_global_enable() };
    let _ = stage0::arm_apic_watchdog(BOOT_PHASE0_WATCHDOG_MS);
    if verify_p0_fixes().is_err() {
        return;
    }
    exoledger::exo_ledger_append(exoledger::ActionTag::BootEvent { step: 0 });
}

pub unsafe fn exoseal_boot_complete() {
    if EXOSEAL_COMPLETE_DONE.swap(true, Ordering::AcqRel) {
        return;
    }

    if verify_p0_fixes().is_err() {
        return;
    }

    // SAFETY: la restauration PKS intervient à la fin du boot sécurité, en ring 0.
    unsafe { exoveil::pks_restore_for_normal_ops(); }

    // SAFETY: `SSR_HANDOFF_FLAG` pointe une case SSR partagée 64-bit, mappée en
    // ring 0 pour ExoPhoenix et utilisée ici uniquement pour revenir en mode normal.
    unsafe {
        ssr::ssr_atomic(ssr::SSR_HANDOFF_FLAG).store(NORMAL_HANDOFF_FLAG, Ordering::Release);
    }
    SECURITY_READY.store(true, Ordering::Release);
    let _ = stage0::arm_apic_watchdog(OPERATIONAL_WATCHDOG_MS);
    exoledger::exo_ledger_append(exoledger::ActionTag::BootEvent { step: 18 });
}

#[inline]
pub fn nic_iommu_locked() -> bool {
    NIC_POLICY_LOCKED.load(Ordering::Acquire)
}

#[inline]
pub fn nic_domain_id() -> u32 {
    NIC_DOMAIN_ID.load(Ordering::Acquire)
}

#[inline]
pub fn nic_dma_window() -> (u64, u64) {
    (
        NIC_DMA_BASE.load(Ordering::Acquire),
        NIC_DMA_END.load(Ordering::Acquire),
    )
}

#[cfg(test)]
mod tests {
    use super::{validate_phase0_state, P0VerifyError};

    #[test]
    fn test_validate_phase0_state_accepts_hardened_state() {
        assert_eq!(
            validate_phase0_state(true, true, true, true, false, true, true, true),
            Ok(())
        );
    }

    #[test]
    fn test_validate_phase0_state_detects_missing_nic_lock() {
        assert_eq!(
            validate_phase0_state(false, true, true, true, false, true, true, true),
            Err(P0VerifyError::NicIommuUnlocked)
        );
    }

    #[test]
    fn test_validate_phase0_state_detects_exposed_caps_domain() {
        assert_eq!(
            validate_phase0_state(true, true, true, true, false, false, true, true),
            Err(P0VerifyError::CapsDomainExposed)
        );
    }
}
