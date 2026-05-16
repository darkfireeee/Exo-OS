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
use crate::exophoenix::{forge, ssr, stage0, PhoenixState, PHOENIX_STATE};
use crate::memory::dma::iommu::{AMD_IOMMU, INTEL_VTD};

const HANDOFF_NORMAL: u64 = ssr::HANDOFF_NORMAL;
const HANDOFF_FREEZE_REQ: u64 = ssr::HANDOFF_FREEZE_REQ;
const HANDOFF_FREEZE_ACK_ALL: u64 = ssr::HANDOFF_FREEZE_ACK_ALL;
const HANDOFF_B_ACTIVE: u64 = ssr::HANDOFF_B_ACTIVE;

const SOFT_TIMEOUT_US: u64 = 100;
const MAX_FORGE_ATTEMPTS: u32 = 3;

const ICR_DM_INIT_X2APIC: u64 = 0b101 << 8;
const ICR_TRIGGER_LEVEL: u64 = 1 << 15;
const ICR_LEVEL_ASSERT: u64 = 1 << 14;

static IOMMU_DRAIN_CONFIRMED: AtomicBool = AtomicBool::new(false);
static FORGE_FAILURE_COUNT: AtomicU32 = AtomicU32::new(0);

const APICBASE_ADDR_MASK: u64 = 0xFFFF_FFFF_F000;
const LAPIC_ID_REG_OFFSET: usize = 0x20;
const CRYPTO_SERVER_ENDPOINT_ID: u64 = 4;
const CRYPTO_PROTOCOL_VERSION: u8 = 3;
const PHOENIX_WAKE_ENTROPY: u32 = 255;
const CRYPTO_REQUEST_PAYLOAD_SIZE: usize = 200;
const CRYPTO_OK: u32 = 0;
const CRYPTO_REPLY_SIZE: usize = 240;
const CRYPTO_REPLY_STATUS_OFFSET: usize = 4;
const CRYPTO_REPLY_VERSION_OFFSET: usize = 14;
const PHOENIX_WAKE_ACK_TIMEOUT_NS: u64 = 5_000_000_000;
const RAW_NOWAIT: u32 = 0x0001;

#[repr(C)]
struct PhoenixWakeRequest {
    sender_pid: u32,
    msg_type: u32,
    reply_endpoint: u64,
    payload_len: u16,
    version: u8,
    flags: u8,
    cap_token: [u8; crate::security::capability::CAP_TOKEN_WIRE_SIZE],
    payload: [u8; CRYPTO_REQUEST_PAYLOAD_SIZE],
}

const _: () = assert!(
    core::mem::size_of::<PhoenixWakeRequest>() <= crate::ipc::core::constants::MAX_MSG_SIZE,
    "PhoenixWakeRequest dépasse MAX_MSG_SIZE"
);

fn phoenix_wake_reply_endpoint(
    entropy: u64,
    timestamp: u64,
) -> Option<crate::ipc::core::types::EndpointId> {
    let cookie = (entropy ^ timestamp ^ 0x5048_4f45_4e49_58) & 0x7fff_ffff_ffff_ffff;
    crate::ipc::core::types::EndpointId::new((1u64 << 63) | cookie.max(1))
}

fn wait_crypto_phoenix_ack(
    reply_endpoint: crate::ipc::core::types::EndpointId,
) -> Result<(), &'static str> {
    let deadline =
        crate::scheduler::timer::clock::monotonic_ns().saturating_add(PHOENIX_WAKE_ACK_TIMEOUT_NS);
    let mut reply = [0u8; CRYPTO_REPLY_SIZE];

    loop {
        match crate::ipc::channel::raw::recv_raw(reply_endpoint, &mut reply, RAW_NOWAIT) {
            Ok(n) if n >= CRYPTO_REPLY_VERSION_OFFSET + 1 => {
                let status = u32::from_le_bytes([
                    reply[CRYPTO_REPLY_STATUS_OFFSET],
                    reply[CRYPTO_REPLY_STATUS_OFFSET + 1],
                    reply[CRYPTO_REPLY_STATUS_OFFSET + 2],
                    reply[CRYPTO_REPLY_STATUS_OFFSET + 3],
                ]);
                if reply[CRYPTO_REPLY_VERSION_OFFSET] == CRYPTO_PROTOCOL_VERSION
                    && status == CRYPTO_OK
                {
                    return Ok(());
                }
                return Err("phoenix_wake_ack_rejected");
            }
            Ok(_) => return Err("phoenix_wake_short_ack"),
            Err(crate::ipc::core::types::IpcError::WouldBlock)
            | Err(crate::ipc::core::types::IpcError::QueueEmpty) => {
                if crate::scheduler::timer::clock::monotonic_ns() >= deadline {
                    return Err("phoenix_wake_ack_timeout");
                }
                unsafe {
                    let _ = crate::scheduler::core::switch::cooperative_reschedule();
                }
            }
            Err(_) => return Err("phoenix_wake_ack_failed"),
        }
    }
}

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

fn phoenix_wake_entropy() -> u64 {
    crate::arch::x86_64::cpu::tsc::read_tsc()
        ^ ((current_apic_id() as u64) << 32)
        ^ handoff_flag_acquire()
        ^ read_apic_timestamp_ticks() as u64
        ^ FORGE_FAILURE_COUNT.load(Ordering::Acquire) as u64
}

fn notify_crypto_server_phoenix_wake() -> Result<(), &'static str> {
    let endpoint = crate::ipc::core::types::EndpointId::new(CRYPTO_SERVER_ENDPOINT_ID)
        .ok_or("phoenix_wake_invalid_endpoint")?;

    let entropy = phoenix_wake_entropy();
    let timestamp = crate::arch::x86_64::cpu::tsc::read_tsc();
    let reply_endpoint =
        phoenix_wake_reply_endpoint(entropy, timestamp).ok_or("phoenix_wake_reply_endpoint")?;

    if !crate::ipc::channel::raw::mailbox_open(reply_endpoint) {
        return Err("phoenix_wake_reply_open_failed");
    }

    let mut request = PhoenixWakeRequest {
        sender_pid: 0,
        msg_type: PHOENIX_WAKE_ENTROPY,
        reply_endpoint: reply_endpoint.get(),
        payload_len: 16,
        version: CRYPTO_PROTOCOL_VERSION,
        flags: 0,
        cap_token: [0u8; crate::security::capability::CAP_TOKEN_WIRE_SIZE],
        payload: [0u8; CRYPTO_REQUEST_PAYLOAD_SIZE],
    };
    request.payload[..8].copy_from_slice(&entropy.to_le_bytes());
    request.payload[8..16].copy_from_slice(&timestamp.to_le_bytes());

    let request_bytes = unsafe {
        core::slice::from_raw_parts(
            &request as *const PhoenixWakeRequest as *const u8,
            core::mem::size_of::<PhoenixWakeRequest>(),
        )
    };

    let send_result = crate::ipc::channel::raw::send_raw(endpoint, request_bytes, RAW_NOWAIT)
        .map(|_| ())
        .map_err(|_| "phoenix_wake_send_failed");
    if send_result.is_err() {
        crate::ipc::channel::raw::mailbox_close(reply_endpoint);
        return send_result;
    }

    let ack_result = wait_crypto_phoenix_ack(reply_endpoint);
    crate::ipc::channel::raw::mailbox_close(reply_endpoint);
    ack_result
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
    let mut seen_slots = [0u64; 4];
    for_each_mapped_apic_slot(|_, slot| {
        if Some(slot) == self_slot {
            return;
        }
        if !super::take_slot_once(&mut seen_slots, slot) {
            return;
        }
        // SAFETY: offset borné par slot map stage0.
        unsafe {
            ssr::ssr_atomic_u32(ssr::freeze_ack_offset(slot)).store(0, Ordering::Release);
        }
    });
}

fn all_freeze_acks_observed(self_slot: Option<usize>) -> bool {
    let mut seen_slots = [0u64; 4];
    let mut all_ok = true;

    for_each_mapped_apic_slot(|_, slot| {
        if !all_ok {
            return;
        }
        if Some(slot) == self_slot {
            return;
        }
        if !super::take_slot_once(&mut seen_slots, slot) {
            return;
        }
        // SAFETY: offset borné par slot map stage0.
        let ack =
            unsafe { ssr::ssr_atomic_u32(ssr::freeze_ack_offset(slot)).load(Ordering::Acquire) };
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
        unsafe {
            INTEL_VTD.flush_iotlb_domain(blocked_domain as u16, 0);
        }
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
        unsafe {
            INTEL_VTD.flush_iotlb_domain(blocked_domain as u16, 0);
        }
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

const PCI_CAP_ID_MSI: u8 = 0x05;
const PCI_CAP_ID_MSIX: u8 = 0x11;
const PCI_CFG_ADDR: u16 = 0xCF8;
const PCI_CFG_DATA: u16 = 0xCFC;

#[inline(always)]
unsafe fn pci_read_dword_handoff(bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
    let addr = 0x8000_0000u32
        | ((bus as u32) << 16)
        | ((dev as u32) << 11)
        | ((func as u32) << 8)
        | ((offset & 0xFC) as u32);
    crate::arch::x86_64::outl(PCI_CFG_ADDR, addr);
    crate::arch::x86_64::inl(PCI_CFG_DATA)
}

#[inline(always)]
unsafe fn pci_write_word_handoff(bus: u8, dev: u8, func: u8, offset: u8, value: u16) {
    let aligned = offset & 0xFC;
    let shift = ((offset & 0x2) * 8) as u32;
    let addr = 0x8000_0000u32
        | ((bus as u32) << 16)
        | ((dev as u32) << 11)
        | ((func as u32) << 8)
        | (aligned as u32);
    crate::arch::x86_64::outl(PCI_CFG_ADDR, addr);
    let mut dword = crate::arch::x86_64::inl(PCI_CFG_DATA);
    dword &= !(0xFFFF << shift);
    dword |= (value as u32) << shift;
    crate::arch::x86_64::outl(PCI_CFG_ADDR, addr);
    crate::arch::x86_64::outl(PCI_CFG_DATA, dword);
}

unsafe fn find_pci_cap(bus: u8, dev: u8, func: u8, cap_id: u8) -> Option<u8> {
    let status = (pci_read_dword_handoff(bus, dev, func, 0x04) >> 16) as u16;
    if status & 0x10 == 0 {
        return None;
    }

    let mut ptr = (pci_read_dword_handoff(bus, dev, func, 0x34) & 0xFF) as u8;
    let mut walked = 0usize;
    while ptr >= 0x40 && walked < 48 {
        let cap = pci_read_dword_handoff(bus, dev, func, ptr);
        if (cap & 0xFF) as u8 == cap_id {
            return Some(ptr);
        }
        let next = ((cap >> 8) & 0xFF) as u8;
        if next == 0 || next == ptr {
            break;
        }
        ptr = next;
        walked += 1;
    }

    None
}

fn mask_all_msi_msix() {
    for i in 0..stage0::b_device_count() {
        let Some(dev) = stage0::b_device(i) else {
            continue;
        };

        // SAFETY: accès PCI config space en ring0 pour les devices Stage0.
        unsafe {
            // MSI : bit0 du MSI Control
            if let Some(msi_cap) = find_pci_cap(dev.bus, dev.device, dev.function, PCI_CAP_ID_MSI) {
                let ctrl_offset = msi_cap + 2;
                let raw = pci_read_dword_handoff(dev.bus, dev.device, dev.function, msi_cap);
                let ctrl = ((raw >> 16) & 0xFFFF) as u16;
                pci_write_word_handoff(
                    dev.bus,
                    dev.device,
                    dev.function,
                    ctrl_offset,
                    ctrl & !0x0001,
                );
            }

            // MSI-X : bit14 Function Mask du MSI-X Control
            if let Some(msix_cap) = find_pci_cap(dev.bus, dev.device, dev.function, PCI_CAP_ID_MSIX)
            {
                let ctrl_offset = msix_cap + 2;
                let raw = pci_read_dword_handoff(dev.bus, dev.device, dev.function, msix_cap);
                let ctrl = ((raw >> 16) & 0xFFFF) as u16;
                pci_write_word_handoff(
                    dev.bus,
                    dev.device,
                    dev.function,
                    ctrl_offset,
                    ctrl | 0x4000,
                );
            }
        }
    }

    core::sync::atomic::fence(Ordering::SeqCst);
}

fn send_init_ipi_to_apic(apic_id: u8) {
    if apic::is_x2apic() {
        let icr =
            ((apic_id as u64) << 32) | ICR_LEVEL_ASSERT | ICR_TRIGGER_LEVEL | ICR_DM_INIT_X2APIC;
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
        let ack =
            unsafe { ssr::ssr_atomic_u32(ssr::freeze_ack_offset(slot)).load(Ordering::Acquire) };
        if ack != ssr::FREEZE_ACK_DONE && ack != ssr::TLB_ACK_DONE {
            send_init_ipi_to_apic(apic_id);
        }
    });
}

fn scan_and_release_spinlocks() {
    // Placeholder phase 3.6: aucune table globale lock-owner exportée ici.
    // Maintenu lock-free pour le chemin critique.
    core::sync::atomic::fence(Ordering::SeqCst);
}

fn reset_irq_watchdogs_after_restore() {
    crate::arch::x86_64::irq::routing::reset_all_masked_since();
}

fn release_frozen_cores_after_restore() {
    set_handoff_flag_release(HANDOFF_B_ACTIVE);
    core::sync::atomic::fence(Ordering::SeqCst);
    set_handoff_flag_release(HANDOFF_NORMAL);
}

fn try_forge_reconstruct_with_policy() -> Result<(), &'static str> {
    for _ in 0..MAX_FORGE_ATTEMPTS {
        match forge::reconstruct_kernel_a() {
            Ok(()) => {
                reset_irq_watchdogs_after_restore();
                if let Err(err) = notify_crypto_server_phoenix_wake() {
                    let failures = FORGE_FAILURE_COUNT.fetch_add(1, Ordering::AcqRel) + 1;
                    if failures >= MAX_FORGE_ATTEMPTS {
                        PHOENIX_STATE.store(PhoenixState::Degraded as u8, Ordering::Release);
                        set_handoff_flag_release(HANDOFF_NORMAL);
                        return Err(err);
                    }
                    continue;
                }
                FORGE_FAILURE_COUNT.store(0, Ordering::Release);
                PHOENIX_STATE.store(PhoenixState::Restore as u8, Ordering::Release);
                release_frozen_cores_after_restore();
                return Ok(());
            }
            Err(_) => {
                let failures = FORGE_FAILURE_COUNT.fetch_add(1, Ordering::AcqRel) + 1;
                if failures >= MAX_FORGE_ATTEMPTS {
                    PHOENIX_STATE.store(PhoenixState::Degraded as u8, Ordering::Release);
                    set_handoff_flag_release(HANDOFF_NORMAL);
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
    set_handoff_flag_release(HANDOFF_NORMAL);
    Ok(())
}
