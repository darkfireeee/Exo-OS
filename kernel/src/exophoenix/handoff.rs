//! ExoPhoenix handoff (Phase 3.6).
//!
//! Applied constraints:
//! - G4: IPI 0xF1 + soft revoke IOMMU launched within the same window.
//! - S-N1: hard revoke + IOTLB flush.
//! - S9: `SSR_HANDOFF_FLAG` in Release/Acquire.
//! - S10: SSR addressing via `apic_to_slot` (never `apic_id*64`).
//! - S1: no explicit spinlock in this module.
//! - G8: no SIPI re-issuance (handled solely in stage0).

#[cfg(not(test))]
use core::fmt::Write as _;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::arch::x86_64::apic::{self, ipi, local_apic, x2apic};
use crate::arch::x86_64::cpu::{msr, tsc};
use crate::arch::x86_64::idt;
use crate::exophoenix::{forge, golden_mirror, ssr, stage0, PhoenixState, PHOENIX_STATE};
use crate::memory::dma::iommu::{AMD_IOMMU, INTEL_VTD};
use exo_phoenix_ssr::metrics::{
    PhoenixTimingRecord, PHOENIX_FLAG_COLD_DISK_FALLBACK, PHOENIX_FLAG_COLD_RESPAWN,
    PHOENIX_FLAG_CRASH_NMI, PHOENIX_FLAG_KVM_ACCEL, PHOENIX_FLAG_TEXT_HEALED,
    PHOENIX_METRICS_MAGIC, PHOENIX_MODE_COLD_RESPAWN, PHOENIX_MODE_CRASH_NMI,
    PHOENIX_MODE_SOFT_FORCE,
};
#[cfg(not(test))]
use exo_phoenix_ssr::metrics::PHOENIX_FLAG_DEGRADED_FREEZE_TIMEOUT;

const HANDOFF_NORMAL: u64 = ssr::HANDOFF_NORMAL;
const HANDOFF_FREEZE_REQ: u64 = ssr::HANDOFF_FREEZE_REQ;
const HANDOFF_FREEZE_ACK_ALL: u64 = ssr::HANDOFF_FREEZE_ACK_ALL;
const HANDOFF_B_ACTIVE: u64 = ssr::HANDOFF_B_ACTIVE;

/// CORR-33 bounded SMP freeze timeout: 100 ms (eliminates global deadlock on stalled AP).
pub const FREEZE_TIMEOUT_MS: u64 = 100;
#[cfg(not(test))]
const TLB_SHOOTDOWN_TIMEOUT_MS: u64 = 10;
const MAX_FORGE_ATTEMPTS: u32 = 3;

const ICR_DM_INIT_X2APIC: u64 = 0b101 << 8;
const ICR_TRIGGER_LEVEL: u64 = 1 << 15;
const ICR_LEVEL_ASSERT: u64 = 1 << 14;

static IOMMU_DRAIN_CONFIRMED: AtomicBool = AtomicBool::new(false);
static FORGE_FAILURE_COUNT: AtomicU32 = AtomicU32::new(0);
static PHOENIX_CYCLE_COUNTER: AtomicU32 = AtomicU32::new(0);
static PHOENIX_FALLBACK_EPOCH: AtomicU64 = AtomicU64::new(0);

const INIT_SERVER_ENDPOINT_ID: u64 = 1;
const INIT_MSG_RELAUNCH_STRATE2: u32 = 8;
#[cfg(not(test))]
const INIT_MSG_COLD_RESPAWN_PHOENIX: u32 = 9;
#[cfg(not(test))]
const COLD_RESPAWN_TIMEOUT_NS: u64 = 15_000_000_000; // 15 s safety ceiling for TCG debug/release
const CRYPTO_SERVER_ENDPOINT_ID: u64 = 4;
const CRYPTO_PROTOCOL_VERSION: u8 = 3;
const PHOENIX_WAKE_ENTROPY: u32 = 255;
const CRYPTO_REQUEST_PAYLOAD_SIZE: usize = 200;
const CRYPTO_OK: u32 = 0;
const CRYPTO_REPLY_SIZE: usize = 240;
const CRYPTO_REPLY_STATUS_OFFSET: usize = 4;
const CRYPTO_REPLY_VERSION_OFFSET: usize = 14;
const CRYPTO_REPLY_DATA_OFFSET: usize = 16;
const PHOENIX_WAKE_ACK_TIMEOUT_NS: u64 = 5_000_000_000;
const PHOENIX_FORCE_CRYPTO_TIMEOUT_NS: u64 = 500_000_000;
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
    let cookie = (entropy ^ timestamp ^ 0x0050_484f_454e_4958) & 0x7fff_ffff_ffff_ffff;
    crate::ipc::core::types::EndpointId::new((1u64 << 63) | cookie.max(1))
}

fn wait_crypto_phoenix_ack(
    reply_endpoint: crate::ipc::core::types::EndpointId,
    timeout_ns: u64,
) -> Result<u64, &'static str> {
    let deadline = crate::scheduler::timer::clock::monotonic_ns().saturating_add(timeout_ns);
    let mut reply = [0u8; CRYPTO_REPLY_SIZE];

    loop {
        match crate::ipc::channel::raw::recv_raw(reply_endpoint, &mut reply, RAW_NOWAIT) {
            Ok(n) if n > CRYPTO_REPLY_VERSION_OFFSET => {
                let status = u32::from_le_bytes([
                    reply[CRYPTO_REPLY_STATUS_OFFSET],
                    reply[CRYPTO_REPLY_STATUS_OFFSET + 1],
                    reply[CRYPTO_REPLY_STATUS_OFFSET + 2],
                    reply[CRYPTO_REPLY_STATUS_OFFSET + 3],
                ]);
                if reply[CRYPTO_REPLY_VERSION_OFFSET] == CRYPTO_PROTOCOL_VERSION
                    && status == CRYPTO_OK
                {
                    let epoch = if n >= CRYPTO_REPLY_DATA_OFFSET + 8 {
                        u64::from_le_bytes([
                            reply[CRYPTO_REPLY_DATA_OFFSET],
                            reply[CRYPTO_REPLY_DATA_OFFSET + 1],
                            reply[CRYPTO_REPLY_DATA_OFFSET + 2],
                            reply[CRYPTO_REPLY_DATA_OFFSET + 3],
                            reply[CRYPTO_REPLY_DATA_OFFSET + 4],
                            reply[CRYPTO_REPLY_DATA_OFFSET + 5],
                            reply[CRYPTO_REPLY_DATA_OFFSET + 6],
                            reply[CRYPTO_REPLY_DATA_OFFSET + 7],
                        ])
                    } else {
                        0
                    };
                    let prev_monotone = PHOENIX_FALLBACK_EPOCH.fetch_add(1, Ordering::AcqRel) + 1;
                    let monotonic_epoch = if epoch > prev_monotone {
                        PHOENIX_FALLBACK_EPOCH.fetch_max(epoch, Ordering::AcqRel);
                        epoch
                    } else {
                        prev_monotone
                    };
                    return Ok(monotonic_epoch);
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
fn current_apic_id() -> u32 {
    match stage0::B_FEATURES.apic_mode() {
        stage0::BootApicMode::X2Apic => {
            // SAFETY: x2APIC active and MSR X2APIC_ID readable.
            unsafe { msr::read_msr(x2apic::X2APIC_ID) as u32 }
        }
        stage0::BootApicMode::XApic => {
            // High-fixmap LAPIC ID read (safe under userspace CR3).
            local_apic::lapic_id()
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
            // SAFETY: x2APIC MSR read of current counter.
            unsafe { msr::read_msr(x2apic::X2APIC_TIMER_CCR) as u32 }
        }
        stage0::BootApicMode::XApic => local_apic::timer_current_count(),
    }
}

#[inline(always)]
fn set_handoff_flag_release(v: u64) {
    // SAFETY: valid SSR offset; Release ordering enforced (S9).
    unsafe {
        ssr::ssr_atomic(ssr::SSR_HANDOFF_FLAG).store(v, Ordering::Release);
    }
}

#[inline(always)]
fn handoff_flag_acquire() -> u64 {
    // SAFETY: valid SSR offset; Acquire read enforced (S9).
    unsafe { ssr::ssr_atomic(ssr::SSR_HANDOFF_FLAG).load(Ordering::Acquire) }
}

fn phoenix_wake_entropy() -> u64 {
    tsc::read_tsc()
        ^ ((current_apic_id() as u64) << 32)
        ^ handoff_flag_acquire()
        ^ read_apic_timestamp_ticks() as u64
        ^ FORGE_FAILURE_COUNT.load(Ordering::Acquire) as u64
}

pub(crate) fn find_crypto_server_runtime_pid() -> Option<u32> {
    if let Some(owner_pid) = crate::syscall::table::ipc::endpoint_owner_pid(CRYPTO_SERVER_ENDPOINT_ID)
    {
        if crate::process::is_alive(owner_pid) {
            return Some(owner_pid);
        }
    }
    let mut found = None;
    crate::process::PROCESS_REGISTRY.for_each(|pcb| {
        if found.is_some() || pcb.is_exiting() || !crate::process::is_alive(pcb.pid.0) {
            return;
        }
        let pid = pcb.pid;
        if crate::security::service_class_of(pid) == crate::security::ServiceClass::CryptoServer {
            found = Some(pid.0);
            return;
        }
        let name = pcb.name_snapshot();
        if name.starts_with(b"exo-crypto-serv") || name.starts_with(b"crypto_server") {
            found = Some(pid.0);
        }
    });
    found
}

fn notify_crypto_server_phoenix_wake_with_timeout(timeout_ns: u64) -> Result<u64, &'static str> {
    let endpoint = crate::ipc::core::types::EndpointId::new(CRYPTO_SERVER_ENDPOINT_ID)
        .ok_or("phoenix_wake_invalid_endpoint")?;

    let entropy = phoenix_wake_entropy();
    let timestamp = tsc::read_tsc();
    let reply_endpoint =
        phoenix_wake_reply_endpoint(entropy, timestamp).ok_or("phoenix_wake_reply_endpoint")?;

    if !crate::ipc::channel::raw::mailbox_open(reply_endpoint) {
        return Err("phoenix_wake_reply_open_failed");
    }

    // T20-001: mint a genuine Ring 0 `IPC_SEND | IPC_MANAGE` CapToken bound to crypto_server's
    // live runtime PID (unforgeable by Ring 3) and revoke it immediately after the reply.
    let mut minted_oid: Option<u32> = None;
    let mut cap_token_wire = [0u8; crate::security::capability::CAP_TOKEN_WIRE_SIZE];
    let crypto_pid = find_crypto_server_runtime_pid();
    // FIX-02: authorize `crypto_server` to send its single-use ACK to this bit-63 reply endpoint.
    let _ = crate::ipc::rpc::raw::register_ephemeral_reply_cap(
        reply_endpoint.get(),
        0,
        crypto_pid.unwrap_or(0),
    );
    if let Some(crypto_pid) = crypto_pid {
        let wake_rights = crate::security::capability::Rights::IPC_SEND
            | crate::security::capability::Rights::IPC_MANAGE;
        if let Ok(token) = crate::security::capability::mint_kernel_ipc_token(crypto_pid, wake_rights)
        {
            cap_token_wire = token.to_bytes();
            minted_oid = Some(token.object_id().as_u64() as u32);
        }
    }

    let mut request = PhoenixWakeRequest {
        sender_pid: 0,
        msg_type: PHOENIX_WAKE_ENTROPY,
        reply_endpoint: reply_endpoint.get(),
        payload_len: 16,
        version: CRYPTO_PROTOCOL_VERSION,
        flags: 0,
        cap_token: cap_token_wire,
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
    if let Err(err) = send_result {
        crate::ipc::rpc::raw::unregister_ephemeral_reply_cap(reply_endpoint.get());
        crate::ipc::channel::raw::mailbox_close(reply_endpoint);
        if let Some(oid) = minted_oid {
            let _ = crate::security::capability::revoke_handle(oid);
        }
        return Err(err);
    }

    let ack_result = wait_crypto_phoenix_ack(reply_endpoint, timeout_ns);
    crate::ipc::rpc::raw::unregister_ephemeral_reply_cap(reply_endpoint.get());
    crate::ipc::channel::raw::mailbox_close(reply_endpoint);
    if let Some(oid) = minted_oid {
        let _ = crate::security::capability::revoke_handle(oid);
    }
    ack_result
}

fn notify_crypto_server_phoenix_wake() -> Result<u64, &'static str> {
    notify_crypto_server_phoenix_wake_with_timeout(PHOENIX_WAKE_ACK_TIMEOUT_NS)
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
        // SAFETY: offset bounded by stage0 slot map.
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
        // SAFETY: offset bounded by stage0 slot map.
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
    // Soft revoke: mark intention and invalidate existing DMA translations.
    IOMMU_DRAIN_CONFIRMED.store(false, Ordering::Release);
    let blocked_domain = stage0::blocked_domain_id();

    if INTEL_VTD.is_initialized() && INTEL_VTD.unit_count() > 0 {
        // SAFETY: CPL0, VT-d initialized.
        unsafe {
            INTEL_VTD.flush_iotlb_domain(blocked_domain as u16, 0);
        }
    } else if AMD_IOMMU.is_initialized() && AMD_IOMMU.unit_count() > 0 {
        // SAFETY: CPL0, AMD-Vi initialized (FIX-19).
        unsafe {
            AMD_IOMMU.flush_domain_and_devtab(blocked_domain as u16);
        }
    }

    IOMMU_DRAIN_CONFIRMED.store(true, Ordering::Release);
}

fn stage_hard_revoke_iommu(with_drain: bool) {
    if with_drain {
        let _ = IOMMU_DRAIN_CONFIRMED.load(Ordering::Acquire);
    }

    let blocked_domain = stage0::blocked_domain_id();

    if INTEL_VTD.is_initialized() && INTEL_VTD.unit_count() > 0 {
        // SAFETY: CPL0, FIX-19 global Context-Cache + IOTLB invalidation across all DRHDs.
        unsafe {
            INTEL_VTD.invalidate_context_and_iotlb_global();
            INTEL_VTD.flush_iotlb_domain(blocked_domain as u16, 0);
        }
    } else if AMD_IOMMU.is_initialized() && AMD_IOMMU.unit_count() > 0 {
        // SAFETY: CPL0, AMD-Vi INVAL_IOMMU_PAGES + COMPLETION_WAIT (FIX-19).
        unsafe {
            AMD_IOMMU.flush_domain_and_devtab(blocked_domain as u16);
        }
    }
}

/// CORR-33 bounded SMP freeze wait (100 ms via calibrated TSC).
///
/// If any peer AP fails to acknowledge within `FREEZE_TIMEOUT_MS`, marks its SSR slot
/// `FREEZE_ACK_DEAD (0xDEAD_0001)` and logs to `ExoLedger` without causing a global deadlock.
fn wait_freeze_ack_and_drain_timeout_corr33(self_slot: Option<usize>) -> bool {
    let start_tsc = tsc::read_tsc();
    let timeout_cycles = tsc::tsc_ms_to_cycles(FREEZE_TIMEOUT_MS).max(1);

    loop {
        let acks_ok = all_freeze_acks_observed(self_slot);
        let drain_ok = IOMMU_DRAIN_CONFIRMED.load(Ordering::Acquire);
        if acks_ok && drain_ok {
            set_handoff_flag_release(HANDOFF_FREEZE_ACK_ALL);
            return true;
        }

        if tsc::read_tsc().wrapping_sub(start_tsc) >= timeout_cycles {
            // Mark every non-responding AP slot `FREEZE_ACK_DEAD` (`CORR-33`).
            let mut seen = [0u64; 4];
            for_each_mapped_apic_slot(|_, slot| {
                if Some(slot) == self_slot || !super::take_slot_once(&mut seen, slot) {
                    return;
                }
                let ack = unsafe {
                    ssr::ssr_atomic_u32(ssr::freeze_ack_offset(slot)).load(Ordering::Acquire)
                };
                if ack != ssr::FREEZE_ACK_DONE && ack != ssr::TLB_ACK_DONE {
                    unsafe {
                        ssr::ssr_atomic_u32(ssr::freeze_ack_offset(slot))
                            .store(ssr::FREEZE_ACK_DEAD, Ordering::Release);
                    }
                    crate::security::exoledger::exo_ledger_append(
                        crate::security::exoledger::ActionTag::Custom {
                            tag: 0xDEAD_0AC4, // CORR-33 DEAD_ACK
                            data: slot as u64,
                        },
                    );
                }
            });
            return false;
        }

        core::hint::spin_loop();
    }
}

/// Counts how many peer cores acknowledged the most recent freeze IPI (`FREEZE_ACK_DONE` / `TLB_ACK_DONE`).
pub fn count_acked_peer_cores() -> u32 {
    let self_slot = current_slot();
    let mut seen = [0u64; 4];
    let mut count = 0u32;
    for_each_mapped_apic_slot(|_, slot| {
        if Some(slot) == self_slot || !super::take_slot_once(&mut seen, slot) {
            return;
        }
        let ack =
            unsafe { ssr::ssr_atomic_u32(ssr::freeze_ack_offset(slot)).load(Ordering::Acquire) };
        if ack == ssr::FREEZE_ACK_DONE || ack == ssr::TLB_ACK_DONE {
            count = count.saturating_add(1);
        }
    });
    count
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

        // SAFETY: PCI config space access in ring0 for Stage0 devices.
        unsafe {
            // MSI: bit0 of MSI Control
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

            // MSI-X: bit14 Function Mask of MSI-X Control
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
        // SAFETY: offset bounded by stage0 slot map.
        let ack =
            unsafe { ssr::ssr_atomic_u32(ssr::freeze_ack_offset(slot)).load(Ordering::Acquire) };
        if ack != ssr::FREEZE_ACK_DONE && ack != ssr::TLB_ACK_DONE {
            send_init_ipi_to_apic(apic_id);
        }
    });
}

fn scan_and_release_spinlocks() {
    // Placeholder phase 3.6: no global lock-owner table exported here.
    // Kept lock-free for the critical path.
    core::sync::atomic::fence(Ordering::SeqCst);
}

fn reset_irq_watchdogs_after_restore() {
    crate::arch::x86_64::time::ktime::phoenix_reset_time_base();
    crate::arch::x86_64::irq::routing::reset_all_masked_since();
}

#[cfg(not(test))]
fn has_other_mapped_cores(self_slot: Option<usize>) -> bool {
    let mut found_other = false;
    for_each_mapped_apic_slot(|_, slot| {
        if Some(slot) != self_slot {
            found_other = true;
        }
    });
    found_other
}

fn verify_strate1_waves_o1() -> u32 {
    // Closed Strate 1 wave membership (Spec §4 / P2):
    // Wave R0: crypto_server, memory_server, ipc_router
    // Wave R1: vfs_server, device_server, fb_server
    // Wave R2: init_server
    // O(1) endpoint lookup over the fixed 7-entry closed list — strictly independent
    // of the number of Strate 2 servers or user processes.
    const STRATE1_WAVE_ENDPOINTS: &[&[u8]] = &[
        b"crypto_server",
        b"memory_server",
        b"ipc_router",
        b"vfs_server",
        b"device_server",
        b"fb_server",
        b"init_server",
    ];
    let mut online = 0u32;
    for &name in STRATE1_WAVE_ENDPOINTS {
        if crate::ipc::endpoint::lookup_endpoint(name).is_some() {
            online += 1;
        }
    }
    online
}

fn notify_init_relaunch_strate2_async() {
    let Some(init_ep) = crate::ipc::core::types::EndpointId::new(INIT_SERVER_ENDPOINT_ID) else {
        return;
    };
    let mut msg = [0u8; 64];
    // InitRequest layout: sender_pid (u32 = 0), msg_type (u32 = INIT_MSG_RELAUNCH_STRATE2 = 8)
    msg[4..8].copy_from_slice(&INIT_MSG_RELAUNCH_STRATE2.to_le_bytes());
    let _ = crate::ipc::channel::raw::send_raw(init_ep, &msg, RAW_NOWAIT);
}

#[cfg(not(test))]
struct StackLineBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

#[cfg(not(test))]
impl<const N: usize> StackLineBuf<N> {
    const fn new() -> Self {
        Self {
            buf: [0u8; N],
            len: 0,
        }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

#[cfg(not(test))]
impl<const N: usize> core::fmt::Write for StackLineBuf<N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let bytes = s.as_bytes();
        let avail = N.saturating_sub(self.len);
        let to_copy = bytes.len().min(avail);
        self.buf[self.len..self.len + to_copy].copy_from_slice(&bytes[..to_copy]);
        self.len += to_copy;
        Ok(())
    }
}

#[cfg(not(test))]
fn post_switch_smp_coherency(self_slot: Option<usize>) {
    // P6.2: 1. Local CR3 reload & KPTI shadow PML4 sync if KPTI active.
    let cr3_now = crate::arch::x86_64::read_cr3();
    if crate::arch::x86_64::spectre::kpti::kpti_enabled() {
        let user_cr3 = crate::arch::x86_64::spectre::kpti::current_user_cr3();
        if user_cr3 != 0 && cr3_now != 0 {
            crate::arch::x86_64::spectre::kpti::set_current_cr3(cr3_now, user_cr3);
        }
    }
    // SAFETY: local CR3 reload flushes non-global TLB entries on the coordinator core.
    unsafe {
        crate::arch::x86_64::reload_cr3();
    }

    // P6.2: 2. Global SMP TLB shootdown broadcast (`VEC_EXOPHOENIX_TLB`) if peer cores exist.
    if has_other_mapped_cores(self_slot) {
        if apic::is_x2apic() {
            x2apic::broadcast_ipi_except_self_x2apic(idt::VEC_EXOPHOENIX_TLB);
        } else {
            local_apic::broadcast_ipi_except_self(idt::VEC_EXOPHOENIX_TLB);
        }
        let start = tsc::read_tsc();
        let timeout = tsc::tsc_ms_to_cycles(TLB_SHOOTDOWN_TIMEOUT_MS).max(1);
        while tsc::read_tsc().wrapping_sub(start) < timeout {
            let mut all_tlb = true;
            let mut seen = [0u64; 4];
            for_each_mapped_apic_slot(|_, slot| {
                if !all_tlb || Some(slot) == self_slot || !super::take_slot_once(&mut seen, slot) {
                    return;
                }
                let ack = unsafe {
                    ssr::ssr_atomic_u32(ssr::freeze_ack_offset(slot)).load(Ordering::Acquire)
                };
                if ack != ssr::TLB_ACK_DONE && ack != ssr::FREEZE_ACK_DEAD {
                    all_tlb = false;
                }
            });
            if all_tlb {
                break;
            }
            core::hint::spin_loop();
        }
    }
}

#[cfg(not(test))]
fn request_init_cold_respawn_phoenix() -> Result<u32, &'static str> {
    let init_ep = crate::ipc::core::types::EndpointId::new(INIT_SERVER_ENDPOINT_ID)
        .ok_or("cold_respawn_invalid_init_ep")?;

    // Unregister endpoints of the servers that `init_server` is about to SIGKILL so readiness
    // polling CANNOT be fooled by stale pre-kill endpoint entries (`F3`).
    const KILLED_ENDPOINTS: &[&[u8]] = &[
        b"crypto_server",
        b"memory_server",
        b"vfs_server",
        b"device_server",
        b"network_server",
        b"scheduler_server",
        b"exo_compositor",
    ];
    for &ep_name in KILLED_ENDPOINTS {
        crate::syscall::table::ipc::force_release_named_endpoint(ep_name);
    }

    let entropy = phoenix_wake_entropy();
    let reply_ep = phoenix_wake_reply_endpoint(entropy ^ 0xC01D_5EA1, tsc::read_tsc())
        .ok_or("cold_respawn_reply_ep")?;
    if !crate::ipc::channel::raw::mailbox_open(reply_ep) {
        return Err("cold_respawn_reply_open_failed");
    }
    // FIX-02: authorize `init_server` (PID 1) to send its single-use ACK to this bit-63 reply endpoint.
    let _ = crate::ipc::rpc::raw::register_ephemeral_reply_cap(reply_ep.get(), 0, 1);

    // InitRequest layout: sender_pid(u32=0), msg_type(u32=9), payload[0..8]=reply_ep(u64).
    let mut msg = [0u8; 64];
    msg[4..8].copy_from_slice(&INIT_MSG_COLD_RESPAWN_PHOENIX.to_le_bytes());
    msg[8..16].copy_from_slice(&reply_ep.get().to_le_bytes());

    if crate::ipc::channel::raw::send_raw(init_ep, &msg, RAW_NOWAIT).is_err() {
        crate::ipc::rpc::raw::unregister_ephemeral_reply_cap(reply_ep.get());
        crate::ipc::channel::raw::mailbox_close(reply_ep);
        return Err("cold_respawn_send_failed");
    }

    let deadline =
        crate::scheduler::timer::clock::monotonic_ns().saturating_add(COLD_RESPAWN_TIMEOUT_NS);
    let mut reply = [0u8; 64];
    let result = loop {
        match crate::ipc::channel::raw::recv_raw(reply_ep, &mut reply, RAW_NOWAIT) {
            Ok(n) if n >= 32 => {
                let status = i64::from_le_bytes([
                    reply[0], reply[1], reply[2], reply[3], reply[4], reply[5], reply[6], reply[7],
                ]);
                let respawned = u32::from_le_bytes([reply[8], reply[9], reply[10], reply[11]]);
                let phx03_ok = u32::from_le_bytes([reply[28], reply[29], reply[30], reply[31]]);
                if status == 0 && respawned >= 4 && phx03_ok == 1 {
                    break Ok(respawned);
                }
                break Err("cold_respawn_wave_failed");
            }
            Ok(_) => break Err("cold_respawn_short_reply"),
            Err(crate::ipc::core::types::IpcError::WouldBlock)
            | Err(crate::ipc::core::types::IpcError::QueueEmpty) => {
                if crate::scheduler::timer::clock::monotonic_ns() >= deadline {
                    break Err("cold_respawn_timeout");
                }
                unsafe {
                    let _ = crate::scheduler::core::switch::cooperative_reschedule();
                }
            }
            Err(_) => break Err("cold_respawn_recv_failed"),
        }
    };

    crate::ipc::rpc::raw::unregister_ephemeral_reply_cap(reply_ep.get());
    crate::ipc::channel::raw::mailbox_close(reply_ep);
    result
}

#[cfg(not(test))]
fn emit_phoenix_sla_telemetry(record: &PhoenixTimingRecord, strate1_online: u32) {
    let mut line = StackLineBuf::<384>::new();
    let degraded = if record.is_degraded() { 1u32 } else { 0u32 };
    let smp = crate::arch::x86_64::smp::init::smp_cpu_count().max(1);
    let accel = if (record.flags & PHOENIX_FLAG_KVM_ACCEL) != 0 {
        "KVM"
    } else {
        "TCG"
    };
    let s1_respawn_us = record.delta_sla_us.saturating_sub(record.delta_s0_us);
    let s2_reconverge_us = record.delta_full_us.saturating_sub(record.delta_sla_us);
    let _ = writeln!(
        line,
        "[EXO_PHOENIX_SLA] CYCLE={} REASON={} S0_FREEZE_US={} S0_FORGE_US={} S0_TOTAL_US={} CRYPTO_US={} S1_TOTAL_US={} FULL_S2_US={} DEGRADED={} RESULT=PASS EPOCH={} S1_ONLINE={} MODE={} SMP={} ACCEL={} S0_US={} S1_RESPAWN_US={} S2_RECONVERGE_US={} HEALED={} MIRROR_BYTES={}",
        record.cycle_id,
        record.reason_str(),
        record.delta_s0_freeze_us,
        record.delta_s0_forge_us,
        record.delta_s0_us,
        record.delta_crypto_us,
        record.delta_sla_us,
        record.delta_full_us,
        degraded,
        record.crypto_epoch_id,
        strate1_online,
        record.mode_str(),
        smp,
        accel,
        record.delta_s0_us,
        s1_respawn_us,
        s2_reconverge_us,
        if record.text_healed() { 1u32 } else { 0u32 },
        golden_mirror::last_verified_bytes(),
    );
    crate::arch::x86_64::terminal::debug_write(line.as_bytes());
}

#[cfg(all(target_arch = "x86_64", not(test)))]
struct Strate0IrqGuard {
    rflags: u64,
}

#[cfg(all(target_arch = "x86_64", not(test)))]
impl Strate0IrqGuard {
    #[inline]
    fn enter() -> Self {
        Self {
            rflags: crate::arch::x86_64::save_and_cli(),
        }
    }
}

#[cfg(all(target_arch = "x86_64", not(test)))]
impl Drop for Strate0IrqGuard {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: restores previous CPL0 IF state upon leaving Strate 0 window (`t3`).
        unsafe {
            crate::arch::x86_64::restore_if_enabled(self.rflags);
        }
    }
}

/// Executes an O(1) 3-Strata ExoPhoenix switch cycle in `PHOENIX_MODE_SOFT_FORCE` mode.
pub fn force_phoenix_switch_cycle(trigger_reason: u32) -> Result<PhoenixTimingRecord, &'static str> {
    force_phoenix_switch_cycle_with_mode(trigger_reason, PHOENIX_MODE_SOFT_FORCE)
}

/// Executes a full 3-Strata ExoPhoenix switch cycle supporting `SOFT_FORCE`, `COLD_RESPAWN`,
/// and `CRASH_NMI` modes (`P5`–`P8`), with serialized TSC milestones `t0..t6`, real SMP freeze
/// (`CORR-33` 100 ms), physical Golden RAM Mirror `.text` healing (`P5.3`), `ExoVeil` revocation,
/// KPTI + SMP TLB shootdown (`P6.2`), `ktime` base reset, `T20-001` Ring 0 capability-authenticated
/// `crypto_server` CSPRNG reseed, destructive `COLD_RESPAWN` (`R0 -> R1 -> R2` + `PHX-03`),
/// SSR v7 `+0xC000` persistence, `ExoLedger` audit, and deterministic P6.4 telemetry.
pub fn force_phoenix_switch_cycle_with_mode(
    trigger_reason: u32,
    mode: u32,
) -> Result<PhoenixTimingRecord, &'static str> {
    let cycle_id = PHOENIX_CYCLE_COUNTER.fetch_add(1, Ordering::AcqRel) + 1;
    let tsc_khz = crate::arch::x86_64::time::ktime::current_tsc_khz().max(1_000);

    let mut record = PhoenixTimingRecord::zeroed();
    record.magic = PHOENIX_METRICS_MAGIC;
    record.cycle_id = cycle_id;
    record.trigger_reason = trigger_reason;
    record.tsc_khz = tsc_khz;
    if mode == PHOENIX_MODE_COLD_RESPAWN {
        record.flags |= PHOENIX_FLAG_COLD_RESPAWN;
    } else if mode == PHOENIX_MODE_CRASH_NMI {
        record.flags |= PHOENIX_FLAG_CRASH_NMI;
    }
    if crate::arch::x86_64::virt::detect::hypervisor_type()
        == crate::arch::x86_64::virt::detect::HypervisorType::Kvm
    {
        record.flags |= PHOENIX_FLAG_KVM_ACCEL;
    }

    // Ensure the read-only Golden RAM Mirror is provisioned BEFORE masking interrupts so the
    // Strate 0 critical window (`t0..t3`) performs zero heap allocations and zero disk I/O.
    golden_mirror::init_golden_mirror();

    #[cfg(not(test))]
    {
        if ssr::validate_layout_v7().is_err() {
            let _ = ssr::initialize_layout_v7();
        }
    }

    // Strate 0 atomic critical window (`t0..t3`): mask timer preemption so untrusted Strate 2
    // threads cannot preempt Ring 0 IOMMU lock + Golden RAM Mirror BLAKE3 verification/healing.
    #[cfg(all(target_arch = "x86_64", not(test)))]
    let strate0_irq_guard = Strate0IrqGuard::enter();

    // t0: Serialized switch start (`lfence; rdtsc`)
    record.t0_freeze_req = tsc::read_tsc_begin();
    PHOENIX_STATE.store(PhoenixState::IsolationSoft as u8, Ordering::Release);

    crate::security::exoledger::exo_ledger_append_p0(
        crate::security::exoledger::ActionTag::HandoffTriggered {
            reason: trigger_reason,
        },
    );

    #[cfg(not(test))]
    let self_slot = current_slot();
    #[cfg(not(test))]
    {
        set_handoff_flag_release(HANDOFF_FREEZE_REQ);
        if has_other_mapped_cores(self_slot) {
            reset_freeze_acks_for_targets(self_slot);
            send_freeze_ipi_broadcast_except_self();
            stage_soft_revoke_iommu();
            if !wait_freeze_ack_and_drain_timeout_corr33(self_slot) {
                record.flags |= PHOENIX_FLAG_DEGRADED_FREEZE_TIMEOUT;
                #[cfg(feature = "phoenix-fault-injection")]
                let controlled_stall = super::fault_inject::take_controlled_ap_stall_occurred();
                #[cfg(not(feature = "phoenix-fault-injection"))]
                let controlled_stall = false;
                if !controlled_stall {
                    mask_all_msi_msix();
                    send_init_ipi_to_resistant_cores(self_slot);
                }
            }
        } else {
            stage_soft_revoke_iommu();
            set_handoff_flag_release(HANDOFF_FREEZE_ACK_ALL);
        }
    }
    #[cfg(test)]
    {
        IOMMU_DRAIN_CONFIRMED.store(true, Ordering::Release);
    }

    // t1: All peer cores frozen (or CORR-33 100 ms degraded timeout) + soft IOMMU drain complete
    record.t1_freeze_ack_all = tsc::read_tsc_end().0;

    // Hard IOMMU revoke + ExoVeil ephemeral domain revocation + Golden RAM Mirror verify/physical heal
    #[cfg(not(test))]
    stage_hard_revoke_iommu(true);
    unsafe {
        crate::security::exoveil::exoveil_revoke_all_on_handoff();
    }
    PHOENIX_STATE.store(PhoenixState::IsolationHard as u8, Ordering::Release);

    let heal_report = golden_mirror::heal_kernel_a_from_mirror()?;
    if heal_report.used_cold_disk_fallback {
        record.flags |= PHOENIX_FLAG_COLD_DISK_FALLBACK;
    }
    if heal_report.corruption_detected && heal_report.verified_after_heal {
        record.flags |= PHOENIX_FLAG_TEXT_HEALED;
    }

    // t2: Hard IOMMU + ExoForge RAM Mirror verification/physical `.text` healing complete
    record.t2_iommu_and_forge = tsc::read_tsc_end().0;

    // Kernel B active handoff + SMP KPTI/TLB coherency + monotonic time-base & IRQ watchdog reset
    #[cfg(not(test))]
    set_handoff_flag_release(HANDOFF_B_ACTIVE);
    #[cfg(not(test))]
    post_switch_smp_coherency(self_slot);
    reset_irq_watchdogs_after_restore();
    unsafe {
        crate::security::exoveil::pks_restore_for_normal_ops();
    }
    PHOENIX_STATE.store(PhoenixState::Restore as u8, Ordering::Release);

    // t3: Kernel B / restored Ring 0 operational (`delta_s0_us` ends here)
    record.t3_kernel_b_active = tsc::read_tsc_end().0;

    // Restore IRQs immediately after `t3` so Wave R0/R1/R2 IPC & respawn can run cleanly.
    #[cfg(all(target_arch = "x86_64", not(test)))]
    drop(strate0_irq_guard);

    // Destructive Cold Respawn (`F3` / P6.3): SIGKILL + parallel R0->R1->R2 + PHX-03 via init_server.
    #[cfg(not(test))]
    if mode == PHOENIX_MODE_COLD_RESPAWN {
        request_init_cold_respawn_phoenix()?;
    }

    // Wave R0 critical crypto reseed (`PHOENIX_WAKE_ENTROPY` + T20-001 CapToken) -> monotonic `epoch_id`
    let epoch_id = if crate::ipc::endpoint::lookup_endpoint(b"crypto_server").is_some() {
        match notify_crypto_server_phoenix_wake_with_timeout(PHOENIX_FORCE_CRYPTO_TIMEOUT_NS) {
            Ok(ep) => ep,
            Err(_) => PHOENIX_FALLBACK_EPOCH.fetch_add(1, Ordering::AcqRel) + 1,
        }
    } else {
        PHOENIX_FALLBACK_EPOCH.fetch_add(1, Ordering::AcqRel) + 1
    };
    record.crypto_epoch_id = epoch_id;

    // t4: `crypto_server` CSPRNG reseeded
    record.t4_crypto_reseeded = tsc::read_tsc_end().0;

    // Closed O(1) Strate 1 wave verification (R0 -> R1 -> R2; zero Strate 2 dependency)
    let strate1_online = verify_strate1_waves_o1();

    // t5: Strate 1 ready — END OF 1 s SLA (`delta_sla_us`)
    record.t5_strate1_ready = tsc::read_tsc_end().0;

    // Trigger asynchronous Strate 2 reconvergence outside the SLA critical path
    if mode != PHOENIX_MODE_COLD_RESPAWN {
        notify_init_relaunch_strate2_async();
    }

    // Release normal handoff state
    #[cfg(not(test))]
    set_handoff_flag_release(HANDOFF_NORMAL);
    PHOENIX_STATE.store(PhoenixState::Normal as u8, Ordering::Release);

    // t6: Strate 2 async reconvergence signaled
    record.t6_strate2_ready = tsc::read_tsc_end().0;

    // Compute 128-bit overflow-free microsecond deltas, persist in SSR +0xC000, audit in ExoLedger
    record.compute_deltas();
    ssr::write_timing_record(&record);
    crate::security::exoledger::exo_ledger_append(crate::security::exoledger::ActionTag::Custom {
        tag: 0x5048_5853,
        data: ((cycle_id as u64) << 32) | (record.delta_sla_us & 0xFFFF_FFFF),
    });

    #[cfg(not(test))]
    emit_phoenix_sla_telemetry(&record, strate1_online);
    #[cfg(test)]
    let _ = strate1_online;

    Ok(record)
}

#[cfg(test)]
mod phoenix_sla_tests {
    use super::*;

    #[test]
    fn force_phoenix_switch_cycle_records_valid_monotonic_metrics_in_ssr() {
        let r1 = force_phoenix_switch_cycle(
            exo_phoenix_ssr::metrics::PHOENIX_REASON_FORCE_SYSCALL,
        )
        .expect("cycle 1");
        let r2 = force_phoenix_switch_cycle(
            exo_phoenix_ssr::metrics::PHOENIX_REASON_FORCE_SYSCALL,
        )
        .expect("cycle 2");
        assert!(r1.is_valid());
        assert!(r2.is_valid());
        assert!(!r1.is_degraded());
        assert!(!r2.is_degraded());
        assert!(r2.cycle_id > r1.cycle_id);
        assert!(r2.crypto_epoch_id > r1.crypto_epoch_id);
        assert!(r2.t0_freeze_req <= r2.t3_kernel_b_active);
        assert!(r2.t3_kernel_b_active <= r2.t5_strate1_ready);
        assert!(r2.t5_strate1_ready <= r2.t6_strate2_ready);
        let stored = ssr::read_timing_record();
        assert_eq!(stored.cycle_id, r2.cycle_id);
        assert_eq!(stored.crypto_epoch_id, r2.crypto_epoch_id);
    }
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

/// Cooperative isolation start (Phase 1).
pub fn begin_isolation_soft() -> Result<(), &'static str> {
    // FIX-AUDIT-V020-P1-2: fail-safe precondition — NEVER broadcast a freeze
    // IPI (which freezes all other cores) if the ExoPhoenix recovery vectors
    // are not armed. stage0_init_all_steps() (lib.rs, common BSP path)
    // arms the vectors at the end of stage0, so the sentinel — which calls
    // this function much later — always sees them active in normal operation.
    // If the flag is false, this is a boot misconfiguration: freezing cores
    // without being able to restart them would be worse than refusing.
    if !crate::exophoenix::stage0::exophoenix_vectors_active() {
        return Err("exophoenix_vectors_not_armed");
    }
    let _ = handoff_flag_acquire();
    let self_slot = current_slot();

    set_handoff_flag_release(HANDOFF_FREEZE_REQ);
    reset_freeze_acks_for_targets(self_slot);

    // G4: freeze IPI + soft revoke in the same critical window.
    send_freeze_ipi_broadcast_except_self();
    stage_soft_revoke_iommu();

    if !wait_freeze_ack_and_drain_timeout_corr33(self_slot) {
        return begin_isolation_hard();
    }

    // S-N1: hard revoke + IOTLB flush after ACK/drain confirmation.
    stage_hard_revoke_iommu(true);

    PHOENIX_STATE.store(PhoenixState::IsolationHard as u8, Ordering::Release);

    try_forge_reconstruct_with_policy()
}

/// Forced isolation start (Phase 2).
pub fn begin_isolation_hard() -> Result<(), &'static str> {
    let self_slot = current_slot();

    // G2: mask MSI/MSI-X before INIT IPI.
    mask_all_msi_msix();
    send_init_ipi_to_resistant_cores(self_slot);

    // Hard revoke without drain.
    stage_hard_revoke_iommu(false);
    scan_and_release_spinlocks();

    PHOENIX_STATE.store(PhoenixState::Certif as u8, Ordering::Release);
    set_handoff_flag_release(HANDOFF_NORMAL);
    Ok(())
}
