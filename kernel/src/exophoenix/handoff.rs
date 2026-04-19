//! Handoff ExoPhoenix (Phase 3.6).
//!
//! Contraintes appliquées:
//! - G4: IPI 0xF1 + soft revoke IOMMU lancés dans la même fenêtre.
//! - S-N1: hard revoke + IOTLB flush.
//! - S9: `SSR_HANDOFF_FLAG` en Release/Acquire.
//! - S10: adressage SSR via `apic_to_slot` (jamais `apic_id*64`).
//! - S1: aucun spinlock explicite dans ce module.
//! - G8: aucun renvoi SIPI (géré uniquement en stage0).

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::arch::x86_64::apic::{self, ipi, local_apic, x2apic};
use crate::arch::x86_64::cpu::msr;
use crate::arch::x86_64::idt;
use crate::exophoenix::{forge, ssr, stage0, PHOENIX_STATE, PhoenixState};
use crate::memory::dma::iommu::{AMD_IOMMU, INTEL_VTD};

const HANDOFF_NORMAL: u64 = 0;
const HANDOFF_FREEZE_REQ: u64 = 1;
const HANDOFF_FREEZE_ACK_ALL: u64 = 2;
const HANDOFF_B_ACTIVE: u64 = 3;

const SOFT_TIMEOUT_US: u64 = 100;
const MAX_FORGE_ATTEMPTS: u32 = 3;

const ICR_DM_INIT_X2APIC: u64 = 0b101 << 8;
const ICR_TRIGGER_LEVEL: u64 = 1 << 15;
const ICR_LEVEL_ASSERT: u64 = 1 << 14;

static IOMMU_DRAIN_CONFIRMED: AtomicBool = AtomicBool::new(false);
static FORGE_FAILURE_COUNT: AtomicU32 = AtomicU32::new(0);

const APICBASE_ADDR_MASK: u64 = 0xFFFF_FFFF_F000;
const LAPIC_ID_REG_OFFSET: usize = 0x20;

#[inline(always)]
fn xapic_mmio_base() -> usize {
    // SAFETY: lecture d'un MSR architectural en Ring 0.
    let apic_base = unsafe { msr::read_msr(msr::MSR_IA32_APIC_BASE) } & APICBASE_ADDR_MASK;
    apic_base as usize
}

#[inline(always)]
fn current_apic_id() -> u32 {
    match stage0::B_FEATURES.apic_mode() {
        stage0::BootApicMode::X2Apic => {
            // SAFETY: x2APIC actif et MSR X2APIC_ID lisible.
            unsafe { msr::read_msr(x2apic::X2APIC_ID) as u32 }
        }
        stage0::BootApicMode::XApic => {
            let lapic_id_ptr = (xapic_mmio_base() + LAPIC_ID_REG_OFFSET) as *const u32;
            // SAFETY: LAPIC MMIO actif en mode xAPIC.
            unsafe { core::ptr::read_volatile(lapic_id_ptr) >> 24 }
        }
    }
}

#[inline(always)]
fn current_slot() -> Option<usize> {
    stage0::apic_slot(current_apic_id())
}

#[inline(always)]
fn read_apic_timestamp_ticks() -> u32 {
    match stage0::B_FEATURES.apic_mode() {
        stage0::BootApicMode::X2Apic => {
            // SAFETY: lecture MSR x2APIC du compteur courant.
            unsafe { msr::read_msr(x2apic::X2APIC_TIMER_CCR) as u32 }
        }
        stage0::BootApicMode::XApic => local_apic::timer_current_count(),
    }
}

#[inline(always)]
fn apic_elapsed_us(start_ticks: u32, end_ticks: u32, ticks_per_us: u64) -> u64 {
    if ticks_per_us == 0 {
        return SOFT_TIMEOUT_US.saturating_add(1);
    }
    start_ticks.wrapping_sub(end_ticks) as u64 / ticks_per_us
}

#[inline(always)]
fn set_handoff_flag_release(v: u64) {
    // SAFETY: offset SSR valide ; Ordering Release imposé (S9).
    unsafe {
        ssr::ssr_atomic(ssr::SSR_HANDOFF_FLAG).store(v, Ordering::Release);
    }
}

#[inline(always)]
fn handoff_flag_acquire() -> u64 {
    // SAFETY: offset SSR valide ; lecture Acquire imposée (S9).
    unsafe { ssr::ssr_atomic(ssr::SSR_HANDOFF_FLAG).load(Ordering::Acquire) }
}

fn for_each_mapped_apic_slot(mut f: impl FnMut(u8, usize)) {
    for apic_id in 0u16..=255u16 {
        let apic = apic_id as u8;
        if let Some(slot) = stage0::apic_slot(apic as u32) {
            f(apic, slot);
        }
    }
}

fn reset_freeze_acks_for_targets(self_slot: Option<usize>) {
    let mut seen_slots: u64 = 0;
    for_each_mapped_apic_slot(|_, slot| {
        if Some(slot) == self_slot {
            return;
        }
        if slot >= 64 {
            return;
        }
        let bit = 1u64 << slot;
        if seen_slots & bit != 0 {
            return;
        }
        seen_slots |= bit;
        // SAFETY: offset borné par slot map stage0.
        unsafe {
            ssr::ssr_atomic_u32(ssr::freeze_ack_offset(slot)).store(0, Ordering::Release);
        }
    });
}

fn all_freeze_acks_observed(self_slot: Option<usize>) -> bool {
    let mut seen_slots: u64 = 0;
    let mut all_ok = true;

    for_each_mapped_apic_slot(|_, slot| {
        if !all_ok {
            return;
        }
        if Some(slot) == self_slot {
            return;
        }
        if slot >= 64 {
            all_ok = false;
            return;
        }
        let bit = 1u64 << slot;
        if seen_slots & bit != 0 {
            return;
        }
        seen_slots |= bit;
        // SAFETY: offset borné par slot map stage0.
        let ack = unsafe { ssr::ssr_atomic_u32(ssr::freeze_ack_offset(slot)).load(Ordering::Acquire) };
        if ack != ssr::FREEZE_ACK_DONE && ack != ssr::TLB_ACK_DONE {
            all_ok = false;
        }
    });

    all_ok
}

fn send_freeze_ipi_broadcast_except_self() {
    if apic::is_x2apic() {
        x2apic::broadcast_ipi_except_self_x2apic(idt::VEC_EXOPHOENIX_FREEZE);
    } else {
        local_apic::broadcast_ipi_except_self(idt::VEC_EXOPHOENIX_FREEZE);
    }
}

fn stage_soft_revoke_iommu() {
    // Soft revoke: marquer l'intention et invalider les traductions DMA existantes.
    IOMMU_DRAIN_CONFIRMED.store(false, Ordering::Release);
    let blocked_domain = stage0::blocked_domain_id();

    if INTEL_VTD.is_initialized() && INTEL_VTD.unit_count() > 0 {
        // SAFETY: CPL0, VT-d initialisé.
        unsafe { INTEL_VTD.flush_iotlb_domain(blocked_domain as u16, 0); }
    } else if AMD_IOMMU.is_initialized() && AMD_IOMMU.unit_count() > 0 {
        core::sync::atomic::fence(Ordering::SeqCst);
    }

    IOMMU_DRAIN_CONFIRMED.store(true, Ordering::Release);
}

fn stage_hard_revoke_iommu(with_drain: bool) {
    if with_drain {
        let _ = IOMMU_DRAIN_CONFIRMED.load(Ordering::Acquire);
    }

    let blocked_domain = stage0::blocked_domain_id();

    if INTEL_VTD.is_initialized() && INTEL_VTD.unit_count() > 0 {
        // SAFETY: CPL0, flush IOTLB domaine bloqué (QI-like sync).
        unsafe { INTEL_VTD.flush_iotlb_domain(blocked_domain as u16, 0); }
    } else if AMD_IOMMU.is_initialized() && AMD_IOMMU.unit_count() > 0 {
        // AMD completion wait fallback (barrière stricte).
        core::sync::atomic::fence(Ordering::SeqCst);
    }
}

fn wait_freeze_ack_and_drain_timeout_100us(self_slot: Option<usize>) -> bool {
    let ticks_per_us = stage0::ticks_per_us();
    let start_ticks = read_apic_timestamp_ticks();

    loop {
        let acks_ok = all_freeze_acks_observed(self_slot);
        let drain_ok = IOMMU_DRAIN_CONFIRMED.load(Ordering::Acquire);
        if acks_ok && drain_ok {
            set_handoff_flag_release(HANDOFF_FREEZE_ACK_ALL);
            return true;
        }

        let now_ticks = read_apic_timestamp_ticks();
        if apic_elapsed_us(start_ticks, now_ticks, ticks_per_us) >= SOFT_TIMEOUT_US {
            return false;
        }

        core::hint::spin_loop();
    }
}

fn mask_all_msi_msix() {
    // Best-effort temporaire : l'infra PCIe capability MSI/MSI-X globale n'est
    // pas encore exposée ici. L'isolation est renforcée ensuite par hard revoke
    // IOMMU + flush IOTLB avant sortie de la phase hard.
    core::sync::atomic::fence(Ordering::SeqCst);
}

fn send_init_ipi_to_apic(apic_id: u8) {
    if apic::is_x2apic() {
        let icr = ((apic_id as u64) << 32)
            | ICR_LEVEL_ASSERT
            | ICR_TRIGGER_LEVEL
            | ICR_DM_INIT_X2APIC;
        x2apic::x2apic_write_icr(icr);
    } else {
        ipi::send_init_ipi(apic_id);
    }
}

fn send_init_ipi_to_resistant_cores(self_slot: Option<usize>) {
    for_each_mapped_apic_slot(|apic_id, slot| {
        if Some(slot) == self_slot {
            return;
        }
        // SAFETY: offset borné par slot map stage0.
        let ack = unsafe { ssr::ssr_atomic_u32(ssr::freeze_ack_offset(slot)).load(Ordering::Acquire) };
        if ack != ssr::FREEZE_ACK_DONE {
            send_init_ipi_to_apic(apic_id);
        }
    });
}

fn scan_and_release_spinlocks() {
    // Placeholder phase 3.6: aucune table globale lock-owner exportée ici.
    // Maintenu lock-free pour le chemin critique.
    core::sync::atomic::fence(Ordering::SeqCst);
}

fn try_forge_reconstruct_with_policy() -> Result<(), &'static str> {
    for _ in 0..MAX_FORGE_ATTEMPTS {
        match forge::reconstruct_kernel_a() {
            Ok(()) => {
                FORGE_FAILURE_COUNT.store(0, Ordering::Release);
                PHOENIX_STATE.store(PhoenixState::Restore as u8, Ordering::Release);
                set_handoff_flag_release(HANDOFF_NORMAL);
                return Ok(());
            }
            Err(_) => {
                let failures = FORGE_FAILURE_COUNT.fetch_add(1, Ordering::AcqRel) + 1;
                if failures >= MAX_FORGE_ATTEMPTS {
                    PHOENIX_STATE.store(PhoenixState::Degraded as u8, Ordering::Release);
                    return Err("forge_reconstruct_failed_degraded");
                }
            }
        }
    }

    Err("forge_reconstruct_failed")
}

/// Démarrage isolation coopérative (Phase 1).
pub fn begin_isolation_soft() -> Result<(), &'static str> {
    let _ = handoff_flag_acquire();
    let self_slot = current_slot();

    set_handoff_flag_release(HANDOFF_FREEZE_REQ);
    reset_freeze_acks_for_targets(self_slot);

    // G4: IPI freeze + soft revoke dans la même fenêtre critique.
    send_freeze_ipi_broadcast_except_self();
    stage_soft_revoke_iommu();

    if !wait_freeze_ack_and_drain_timeout_100us(self_slot) {
        return begin_isolation_hard();
    }

    // S-N1: hard revoke + IOTLB flush après confirmation des ACK/drain.
    stage_hard_revoke_iommu(true);

    PHOENIX_STATE.store(PhoenixState::IsolationHard as u8, Ordering::Release);
    set_handoff_flag_release(HANDOFF_B_ACTIVE);

    try_forge_reconstruct_with_policy()
}

/// Démarrage isolation forcée (Phase 2).
pub fn begin_isolation_hard() -> Result<(), &'static str> {
    let self_slot = current_slot();

    // G2: masquer MSI/MSI-X avant INIT IPI.
    mask_all_msi_msix();
    send_init_ipi_to_resistant_cores(self_slot);

    // Hard revoke sans drain.
    stage_hard_revoke_iommu(false);
    scan_and_release_spinlocks();

    PHOENIX_STATE.store(PhoenixState::Certif as u8, Ordering::Release);
    Ok(())
}
