//! # arch/aarch64 — AArch64 Placeholder
//!
//! Minimal architecture module for AArch64 targets (64-bit ARMv8-A).
//!
//! ## Status
//! ExoOS v0.2.0 does not declare AArch64 as a supported boot target: no
//! full VBAR/EL1, no GIC/APIC equivalent, no SMP or AArch64 syscall ABI.
//! The primitives below remain localized for a future port;
//! target rejection happens at the crate level, not in this module.

// ── Basic Primitives ─────────────────────────────────────────────────────────

/// Reads the time counter (CNTVCT_EL0)
#[inline(always)]
pub fn read_tsc() -> u64 {
    let val: u64;
    // SAFETY: CNTVCT_EL0 is readable from EL0/EL1 without restriction
    unsafe {
        core::arch::asm!("mrs {}, cntvct_el0", out(reg) val, options(nostack, nomem));
    }
    val
}

/// Halts the CPU (WFI — Wait For Interrupt)
#[inline(always)]
pub fn halt_cpu() -> ! {
    loop {
        // SAFETY: WFI is a wait instruction — wake up on interrupt
        unsafe {
            core::arch::asm!("wfi", options(nostack, nomem));
        }
    }
}

/// Disables interrupts (DAIF.I = 1)
#[inline(always)]
pub fn irq_disable() {
    // SAFETY: writing DAIF register from EL1
    unsafe {
        core::arch::asm!("msr daifset, #2", options(nostack, nomem));
    }
}

/// Enables interrupts (DAIF.I = 0)
#[inline(always)]
pub fn irq_enable() {
    // SAFETY: writing DAIF register from EL1
    unsafe {
        core::arch::asm!("msr daifclr, #2", options(nostack, nomem));
    }
}

/// Saves and disables interrupts
#[inline(always)]
pub fn irq_save() -> u64 {
    let daif: u64;
    // SAFETY: reading DAIF
    unsafe {
        core::arch::asm!("mrs {}, daif", out(reg) daif, options(nostack, nomem));
    }
    irq_disable();
    daif
}

/// Restores interrupt state
#[inline(always)]
pub fn irq_restore(daif: u64) {
    // SAFETY: restoring previously saved DAIF
    unsafe {
        core::arch::asm!("msr daif, {}", in(reg) daif, options(nostack, nomem));
    }
}

/// Full memory barrier (DMB ISH)
#[inline(always)]
pub fn memory_barrier() {
    // SAFETY: barrier instruction — no side effects on CPU state
    unsafe {
        core::arch::asm!("dmb ish", options(nostack));
    }
}

/// Load barrier (DMB ISHLD)
#[inline(always)]
pub fn load_fence() {
    // SAFETY: load-load barrier
    unsafe {
        core::arch::asm!("dmb ishld", options(nostack));
    }
}

/// Store barrier (DMB ISHST)
#[inline(always)]
pub fn store_fence() {
    // SAFETY: store-store barrier
    unsafe {
        core::arch::asm!("dmb ishst", options(nostack));
    }
}

/// Invalidates a TLB page at the given virtual address
#[inline(always)]
pub fn flush_tlb_page(virt_addr: u64) {
    // SAFETY: TLBI VAAE1IS invalidates a shared EL1 TLB entry
    unsafe {
        core::arch::asm!(
            "tlbi vaae1is, {}",
            "dsb ish",
            "isb",
            in(reg) virt_addr >> 12,
            options(nostack, nomem),
        );
    }
}

/// Invalidates the entire EL1 TLB (all ASIDs)
#[inline(always)]
pub fn flush_tlb() {
    // SAFETY: TLBI VMALLE1IS flushes the entire EL1 TLB
    unsafe {
        core::arch::asm!("tlbi vmalle1is", "dsb ish", "isb", options(nostack, nomem),);
    }
}

/// Active busy-wait delay in cycles (based on CNTVCT_EL0)
pub fn spin_delay_cycles(cycles: u64) {
    let start = read_tsc();
    while read_tsc().wrapping_sub(start) < cycles {
        core::hint::spin_loop();
    }
}

// ── ArchInfo ─────────────────────────────────────────────────────────────────

use super::ArchInfo;

/// Returns AArch64 architecture information
pub fn arch_info() -> ArchInfo {
    ArchInfo {
        cpu_count: 1,
        has_apic: false,
        has_x2apic: false,
        has_acpi: false,
        page_size: 4096,
    }
}

/// Reads the generic counter frequency (CNTFRQ_EL0)
fn read_cntfrq() -> u64 {
    let val: u32;
    // SAFETY: CNTFRQ_EL0 readable from EL0/EL1
    unsafe {
        core::arch::asm!("mrs {:x}, cntfrq_el0", out(reg) val, options(nostack, nomem));
    }
    val as u64
}
