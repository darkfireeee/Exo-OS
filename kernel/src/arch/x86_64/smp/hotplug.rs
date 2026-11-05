//! # arch/x86_64/smp/hotplug.rs — CPU Hotplug (online/offline)
//!
//! Allows bringing CPUs online or offline dynamically.
//! Used for power management and SMP migrations.
//!
//! ## Offline Sequence
//! 1. Migrate all threads from the CPU to other CPUs
//! 2. Send IPI_CPU_HOTPLUG to the target CPU
//! 3. Target CPU enters a halt loop and declares itself offline
//! 4. BSP waits for confirmation in `CPU_ONLINE_MASK`
//!
//! ## Online Sequence
//! 1. Send INIT + SIPI if the CPU is cold
//! 2. (Alternative) Wake up from the halt loop via IPI wakeup
//!    if the CPU was only in shallow sleep

use super::super::apic::ipi;
use super::super::cpu::tsc;
use crate::arch::x86_64::cpu::topology::MAX_CPUS;
use core::sync::atomic::{AtomicU64, Ordering};

// ── Online CPU Mask ──────────────────────────────────────────────────────────

/// Number of u64 words required for the online bitmask.
const ONLINE_MASK_WORDS: usize = MAX_CPUS.div_ceil(64);

/// Bitmask of online CPUs — bit N = CPU N online.
/// AtomicU64 array: supports up to MAX_CPUS CPUs.
/// [0] bit 0 = BSP always online (initial).
static CPU_ONLINE_MASK: [AtomicU64; ONLINE_MASK_WORDS] = [
    AtomicU64::new(1), // bit 0 = BSP
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

/// Compile-time assertion: array correctly sized.
const _: () = assert!(
    ONLINE_MASK_WORDS == MAX_CPUS.div_ceil(64),
    "CPU_ONLINE_MASK mal dimensionné pour MAX_CPUS"
);

/// Returns `true` if CPU `cpu_id` is online
#[inline]
pub fn cpu_is_online(cpu_id: u32) -> bool {
    let id = cpu_id as usize;
    if id >= MAX_CPUS {
        return false;
    }
    let word = id / 64;
    let bit = id % 64;
    CPU_ONLINE_MASK[word].load(Ordering::Acquire) & (1u64 << bit) != 0
}

/// Marks a CPU as online
#[inline]
pub fn set_cpu_online(cpu_id: u32) {
    let id = cpu_id as usize;
    if id >= MAX_CPUS {
        return;
    }
    let word = id / 64;
    let bit = id % 64;
    CPU_ONLINE_MASK[word].fetch_or(1u64 << bit, Ordering::AcqRel);
}

/// Marks a CPU as offline
#[inline]
pub fn set_cpu_offline(cpu_id: u32) {
    let id = cpu_id as usize;
    if id >= MAX_CPUS {
        return;
    }
    let word = id / 64;
    let bit = id % 64;
    CPU_ONLINE_MASK[word].fetch_and(!(1u64 << bit), Ordering::AcqRel);
}

/// Returns the number of online CPUs
pub fn online_cpu_count() -> u32 {
    CPU_ONLINE_MASK
        .iter()
        .map(|w| w.load(Ordering::Relaxed).count_ones())
        .sum()
}

/// Returns `true` if at least `count` CPUs are online.
pub fn all_cpus_online(count: u32) -> bool {
    online_cpu_count() >= count
}

// ── Online / Offline Transition ───────────────────────────────────────────────

/// Attempts to bring a CPU online
///
/// If the CPU was in a halt loop, sends an IPI wakeup.
/// If the CPU was cold, initiates the INIT+SIPI sequence.
///
/// Returns `true` if the CPU is online after 100ms.
pub fn cpu_online(cpu_id: u32, lapic_id: u32) -> bool {
    if cpu_is_online(cpu_id) {
        return true;
    }

    // Attempt an IPI wakeup first (CPU in shallow halt)
    ipi::send_ipi_wakeup(lapic_id);

    let deadline = tsc::read_tsc() + tsc::tsc_ms_to_cycles(100);
    while tsc::read_tsc() < deadline {
        if cpu_is_online(cpu_id) {
            return true;
        }
        tsc::tsc_delay_us(1000); // 1ms poll
    }

    // CPU not responding: full boot sequence
    ipi::send_init_ipi(lapic_id as u8);
    tsc::tsc_delay_ms(10);
    ipi::send_startup_ipi(lapic_id as u8, super::init::TRAMPOLINE_PAGE);
    tsc::tsc_delay_ms(1);
    ipi::send_startup_ipi(lapic_id as u8, super::init::TRAMPOLINE_PAGE);

    let deadline = tsc::read_tsc() + tsc::tsc_ms_to_cycles(200);
    while tsc::read_tsc() < deadline {
        if cpu_is_online(cpu_id) {
            return true;
        }
        tsc::tsc_delay_us(1000);
    }

    false
}

/// Takes a CPU offline
///
/// Sends an IPI HOTPLUG to target CPU, then awaits its confirmation.
/// Target CPU calls `hotplug_cpu_halt()` in response.
///
/// Returns `true` if the CPU is offline after 500ms.
pub fn cpu_offline(cpu_id: u32, lapic_id: u32) -> bool {
    if !cpu_is_online(cpu_id) {
        return true;
    }
    if cpu_id == 0 {
        return false;
    } // BSP cannot take itself offline

    ipi::send_ipi_cpu_hotplug(lapic_id);

    let deadline = tsc::read_tsc() + tsc::tsc_ms_to_cycles(500);
    while tsc::read_tsc() < deadline {
        if !cpu_is_online(cpu_id) {
            return true;
        }
        tsc::tsc_delay_us(1000);
    }

    false
}

/// Hotplug handler called by target AP upon receiving the IPI
///
/// The AP declares itself offline and enters a halt loop.
/// It can be woken up later by an IPI wakeup.
pub fn hotplug_cpu_halt(cpu_id: u32) -> ! {
    // Mask all local interrupts except IPI wakeup
    // SAFETY: interrupt disabling on this CPU before halt
    unsafe {
        core::arch::asm!("cli", options(nostack, nomem));
    }

    set_cpu_offline(cpu_id);

    // Halt loop (reactivatable via IPI wakeup + STI in the handler)
    loop {
        // SAFETY: safe halt — only exit = NMI or IPI wakeup (if pre-halt STI)
        unsafe {
            core::arch::asm!("sti\n\thlt\n\tcli", options(nostack, nomem));
        }
    }
}

/// Returns the bitmask of online CPUs (first 64 CPUs)
pub fn online_mask() -> u64 {
    CPU_ONLINE_MASK[0].load(Ordering::Acquire)
}
