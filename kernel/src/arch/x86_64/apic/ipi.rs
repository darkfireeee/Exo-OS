//! # arch/x86_64/apic/ipi.rs — Inter-Processor Interrupts
//!
//! Sending IPIs to other CPUs for SMP synchronization.
//!
//! ## IPI Vectors (defined in idt.rs)
//! - 0xE0 `VEC_IPI_RESCHEDULE`: force a reschedule
//! - 0xE1 `VEC_IPI_WAKEUP`: wake up a thread on a CPU
//! - 0xE2 `VEC_IPI_TLB_SHOOTDOWN`: invalidate a TLB page
//! - 0xE3 `VEC_IPI_CPU_HOTPLUG`: CPU hotplug notification
//! - 0xFE `VEC_IPI_PANIC`: emergency stop (broadcast)
//!
//! ## Routing
//! Delegated to x2APIC (MSR ICR) or xAPIC (MMIO ICR) according to active mode.

use super::super::idt::{
    VEC_IPI_CPU_HOTPLUG, VEC_IPI_PANIC, VEC_IPI_RESCHEDULE, VEC_IPI_TLB_SHOOTDOWN, VEC_IPI_WAKEUP,
};
use core::sync::atomic::{AtomicU64, Ordering};

// ── TLB Shootdown payload ─────────────────────────────────────────────────────

/// Target virtual address of ongoing TLB shootdown (0 = full flush)
static TLB_SHOOTDOWN_ADDR: AtomicU64 = AtomicU64::new(0);
/// Counter of CPUs that acknowledged the current TLB shootdown
static TLB_SHOOTDOWN_ACK: AtomicU64 = AtomicU64::new(0);
/// TLB shootdown generation (anti-spurious)
static TLB_SHOOTDOWN_GEN: AtomicU64 = AtomicU64::new(0);

// ── Generic IPI Send ──────────────────────────────────────────────────────────

/// Sends an IPI to a specific APIC ID (auto-selection xAPIC/x2APIC)
#[inline]
fn send_ipi_to(dest_apic_id: u32, vector: u8) {
    if super::is_x2apic() {
        super::x2apic::send_ipi_x2apic(dest_apic_id, vector, 0);
    } else {
        super::local_apic::send_ipi(dest_apic_id as u8, vector, super::local_apic::ICR_DM_FIXED);
    }
}

/// Broadcasts IPI to all CPUs EXCEPT self
#[inline]
fn broadcast_ipi_except_self(vector: u8) {
    if super::is_x2apic() {
        super::x2apic::broadcast_ipi_except_self_x2apic(vector);
    } else {
        super::local_apic::broadcast_ipi_except_self(vector);
    }
}

// ── Specialized IPIs ─────────────────────────────────────────────────────────

/// Sends a wakeup IPI to a target CPU (to wake up a thread)
///
/// Used by `scheduler::smp::wakeup::wake_cpu()`.
pub fn send_ipi_wakeup(dest_apic_id: u32) {
    IPI_WAKEUP_SENT.fetch_add(1, Ordering::Relaxed);
    send_ipi_to(dest_apic_id, VEC_IPI_WAKEUP);
}

/// Sends a reschedule IPI to a target CPU (SMP preemption)
///
/// Used by `scheduler::smp::preempt::kick_cpu()`.
pub fn send_ipi_reschedule(dest_apic_id: u32) {
    IPI_RESCHEDULE_SENT.fetch_add(1, Ordering::Relaxed);
    send_ipi_to(dest_apic_id, VEC_IPI_RESCHEDULE);
}

/// Sends a TLB shootdown IPI to a target CPU
///
/// - `virt_addr = 0`: full TLB flush (`INVLPG` on all CPUs)
/// - `virt_addr ≠ 0`: flush of a specific page
///
/// Called by `memory::virtual::address_space::tlb::shootdown_page()`.
pub fn send_ipi_tlb_shootdown(dest_apic_id: u32, virt_addr: u64) {
    TLB_SHOOTDOWN_ADDR.store(virt_addr, Ordering::Release);
    TLB_SHOOTDOWN_GEN.fetch_add(1, Ordering::AcqRel);
    IPI_TLB_SENT.fetch_add(1, Ordering::Relaxed);
    send_ipi_to(dest_apic_id, VEC_IPI_TLB_SHOOTDOWN);
}

/// Broadcasts TLB shootdown to all CPUs EXCEPT self
///
/// Waits for ACK return from all CPUs (100µs timeout by default).
pub fn broadcast_tlb_shootdown(virt_addr: u64, cpu_count: u32) {
    if cpu_count <= 1 {
        return;
    }

    TLB_SHOOTDOWN_ADDR.store(virt_addr, Ordering::Release);
    TLB_SHOOTDOWN_ACK.store(0, Ordering::Release);
    let gen = TLB_SHOOTDOWN_GEN.fetch_add(1, Ordering::AcqRel) + 1;
    IPI_TLB_SENT.fetch_add(1, Ordering::Relaxed);

    broadcast_ipi_except_self(VEC_IPI_TLB_SHOOTDOWN);

    // Wait for ACK from (cpu_count - 1) remote CPUs
    let expected = cpu_count.saturating_sub(1) as u64;
    let deadline =
        super::super::cpu::tsc::read_tsc() + super::super::cpu::tsc::tsc_us_to_cycles(100);

    while TLB_SHOOTDOWN_ACK.load(Ordering::Acquire) < expected {
        if super::super::cpu::tsc::read_tsc() > deadline {
            // Timeout: continue anyway (some CPUs may be halted)
            break;
        }
        core::hint::spin_loop();
    }
    let _ = gen;
}

/// Acknowledges a received TLB shootdown (called by IPI handler in exceptions.rs)
#[inline]
pub fn ack_tlb_shootdown() {
    TLB_SHOOTDOWN_ACK.fetch_add(1, Ordering::Release);
}

/// Returns address of ongoing TLB shootdown
#[inline]
pub fn tlb_shootdown_addr() -> u64 {
    TLB_SHOOTDOWN_ADDR.load(Ordering::Acquire)
}

/// Sends a CPU hotplug IPI
///
/// Used to notify a CPU of an online/offline change.
pub fn send_ipi_cpu_hotplug(dest_apic_id: u32) {
    IPI_HOTPLUG_SENT.fetch_add(1, Ordering::Relaxed);
    send_ipi_to(dest_apic_id, VEC_IPI_CPU_HOTPLUG);
}

/// Broadcasts panic IPI to all CPUs (emergency stop)
///
/// Called during a kernel panic to stop all APs immediately.
/// **Do NOT call from an IPI handler.**
pub fn broadcast_ipi_panic() {
    IPI_PANIC_SENT.fetch_add(1, Ordering::Relaxed);
    broadcast_ipi_except_self(VEC_IPI_PANIC);
}

/// Sends INIT IPI to an AP (SMP startup sequence)
///
/// Delegated to `local_apic::send_init_ipi()`.
pub fn send_init_ipi(dest_apic_id: u8) {
    super::local_apic::send_init_ipi(dest_apic_id);
}

/// Sends STARTUP IPI (SIPI) to an AP
///
/// `page` = 4K page number of startup vector (trampoline).
/// E.g.: trampoline at 0x6000 → page = 6.
pub fn send_startup_ipi(dest_apic_id: u8, trampoline_page: u8) {
    super::local_apic::send_startup_ipi(dest_apic_id, trampoline_page);
}

// ── Instrumentation ───────────────────────────────────────────────────────────

static IPI_WAKEUP_SENT: AtomicU64 = AtomicU64::new(0);
static IPI_RESCHEDULE_SENT: AtomicU64 = AtomicU64::new(0);
static IPI_TLB_SENT: AtomicU64 = AtomicU64::new(0);
static IPI_HOTPLUG_SENT: AtomicU64 = AtomicU64::new(0);
static IPI_PANIC_SENT: AtomicU64 = AtomicU64::new(0);

pub fn ipi_wakeup_sent() -> u64 {
    IPI_WAKEUP_SENT.load(Ordering::Relaxed)
}
pub fn ipi_reschedule_sent() -> u64 {
    IPI_RESCHEDULE_SENT.load(Ordering::Relaxed)
}
pub fn ipi_tlb_sent() -> u64 {
    IPI_TLB_SENT.load(Ordering::Relaxed)
}
pub fn ipi_hotplug_sent() -> u64 {
    IPI_HOTPLUG_SENT.load(Ordering::Relaxed)
}
pub fn ipi_panic_sent() -> u64 {
    IPI_PANIC_SENT.load(Ordering::Relaxed)
}
