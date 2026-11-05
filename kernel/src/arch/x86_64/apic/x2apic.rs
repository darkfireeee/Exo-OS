//! # arch/x86_64/apic/x2apic.rs — x2APIC (MSR mode)
//!
//! The x2APIC mode replaces xAPIC MMIO access with MSRs.
//! APIC ID address space increases from 8-bit to 32-bit.
//!
//! ## x2APIC MSR Range
//! LAPIC registers are accessed via MSRs 0x800–0x8FF.
//! Each MMIO register at offset 0xXY0 becomes MSR 0x800 + (0xXY0 >> 4).

use super::super::cpu::msr;

// ── x2APIC MSR Constants ──────────────────────────────────────────────────────

const X2APIC_BASE: u32 = 0x800;

pub const X2APIC_ID: u32 = X2APIC_BASE + 0x02; // 0x802
pub const X2APIC_VER: u32 = X2APIC_BASE + 0x03;
pub const X2APIC_TPR: u32 = X2APIC_BASE + 0x08;
pub const X2APIC_PPR: u32 = X2APIC_BASE + 0x0A;
pub const X2APIC_EOI: u32 = X2APIC_BASE + 0x0B;
pub const X2APIC_LDR: u32 = X2APIC_BASE + 0x0D;
pub const X2APIC_SIVR: u32 = X2APIC_BASE + 0x0F;
pub const X2APIC_ISR0: u32 = X2APIC_BASE + 0x10;
pub const X2APIC_ESR: u32 = X2APIC_BASE + 0x28;
pub const X2APIC_LVT_CMCI: u32 = X2APIC_BASE + 0x2F;
pub const X2APIC_ICR: u32 = X2APIC_BASE + 0x30; // 64-bit ICR (combined hi+lo)
pub const X2APIC_LVT_TIMER: u32 = X2APIC_BASE + 0x32;
pub const X2APIC_LVT_THERMAL: u32 = X2APIC_BASE + 0x33;
pub const X2APIC_LVT_PERF: u32 = X2APIC_BASE + 0x34;
pub const X2APIC_LVT_LINT0: u32 = X2APIC_BASE + 0x35;
pub const X2APIC_LVT_LINT1: u32 = X2APIC_BASE + 0x36;
pub const X2APIC_LVT_ERROR: u32 = X2APIC_BASE + 0x37;
pub const X2APIC_TIMER_ICR: u32 = X2APIC_BASE + 0x38;
pub const X2APIC_TIMER_CCR: u32 = X2APIC_BASE + 0x39;
pub const X2APIC_TIMER_DCR: u32 = X2APIC_BASE + 0x3E;
pub const X2APIC_SELF_IPI: u32 = X2APIC_BASE + 0x3F; // Self-IPI (x2APIC only)

const MSR_IA32_APICBASE: u32 = 0x001B;
const APICBASE_ENABLE: u64 = 1 << 11;
const APICBASE_EXTD: u64 = 1 << 10;

// ── MSR Read / Write ──────────────────────────────────────────────────────────

/// Reads an x2APIC register (32-bit)
#[inline(always)]
pub fn x2apic_read32(reg: u32) -> u32 {
    // SAFETY: validated x2APIC register, Ring 0 only
    unsafe { msr::read_msr(reg) as u32 }
}

/// Reads an x2APIC register (64-bit, ICR only)
#[inline(always)]
pub fn x2apic_read64(reg: u32) -> u64 {
    // SAFETY: reg must be X2APIC_ICR for 64-bit access
    unsafe { msr::read_msr(reg) }
}

/// Writes an x2APIC register (32-bit)
#[inline(always)]
pub fn x2apic_write32(reg: u32, val: u32) {
    // SAFETY: validated x2APIC register, Ring 0 only
    unsafe {
        msr::write_msr(reg, val as u64);
    }
}

/// Writes 64-bit ICR (single IPI send — no waiting for delivery status in x2APIC)
#[inline(always)]
pub fn x2apic_write_icr(val: u64) {
    // SAFETY: MSR X2APIC_ICR is write-only in x2APIC
    unsafe {
        msr::write_msr(X2APIC_ICR, val);
    }
}

// ── x2APIC Activation ─────────────────────────────────────────────────────────

/// Masks all LVTs in x2APIC mode.
///
/// Must be called right after `enable_x2apic()` to ensure that
/// LVT entries left by the BIOS (e.g. LINT0 with vector 0x8E) cannot
/// deliver interrupts to unregistered IDT vectors → #GP.
pub fn mask_all_lvt_x2apic() {
    const MASKED: u32 = 0x0001_0000; // bit 16 = mask
    x2apic_write32(X2APIC_LVT_THERMAL, MASKED);
    x2apic_write32(X2APIC_LVT_PERF, MASKED);
    x2apic_write32(X2APIC_LVT_CMCI, MASKED);
    x2apic_write32(X2APIC_LVT_LINT0, MASKED);
    x2apic_write32(X2APIC_LVT_LINT1, 0x0000_0400); // NMI, unmasked
    x2apic_write32(X2APIC_LVT_ERROR, 0x0000_00FE); // vector 0xFE, unmasked
                                                   // Clear ESR
    x2apic_write32(X2APIC_ESR, 0);
    x2apic_write32(X2APIC_ESR, 0);
    // Task Priority: accept all interrupts
    x2apic_write32(X2APIC_TPR, 0);
}

/// Enables x2APIC mode (bits 10+11 of MSR IA32_APICBASE)
///
/// Transition: xAPIC (bit 11 only) → x2APIC (bits 11+10)
/// **Irreversible**: once enabled, returning to xAPIC requires a reset.
pub fn enable_x2apic() {
    // SAFETY: xAPIC → x2APIC transition; Ring 0, CPU supports x2APIC (verified by cpu::features)
    let apicbase = unsafe { msr::read_msr(MSR_IA32_APICBASE) };
    // SAFETY: write to MSR APICBASE to enable x2APIC
    unsafe {
        msr::write_msr(
            MSR_IA32_APICBASE,
            apicbase | APICBASE_ENABLE | APICBASE_EXTD,
        );
    }

    // Soft LAPIC enable via SIVR
    let sivr = x2apic_read32(X2APIC_SIVR) | 0x100;
    x2apic_write32(X2APIC_SIVR, sivr);
}

// ── x2APIC Primitives ─────────────────────────────────────────────────────────

/// EOI in x2APIC mode
#[inline(always)]
pub fn eoi_x2apic() {
    x2apic_write32(X2APIC_EOI, 0);
}

/// 32-bit LAPIC ID in x2APIC mode
#[inline]
pub fn x2apic_id() -> u32 {
    x2apic_read32(X2APIC_ID)
}

/// Sends an IPI in x2APIC mode
///
/// ICR is a single 64-bit register — no delivery status to wait for.
/// x2APIC ICR format:
///   bits 7:0   = vector
///   bits 10:8  = delivery mode
///   bit  11    = destination mode (0=physical, 1=logical)
///   bit  14    = level (1=assert)
///   bit  15    = trigger mode (0=edge, 1=level)
///   bits 19:18 = destination shorthand
///   bits 63:32 = destination (32-bit APIC ID)
pub fn send_ipi_x2apic(dest: u32, vector: u8, delivery_mode: u64) {
    let icr: u64 = ((dest as u64) << 32)
        | (1 << 14)          // level assert
        | delivery_mode
        | (vector as u64);
    x2apic_write_icr(icr);
}

/// Broadcasts IPI to all except self in x2APIC
pub fn broadcast_ipi_except_self_x2apic(vector: u8) {
    // shorthand 0b11 = all excluding self
    let icr: u64 = (0b11u64 << 18) | (1 << 14) | (vector as u64);
    x2apic_write_icr(icr);
}

/// Self-IPI in x2APIC (dedicated register MSR 0x83F)
#[inline]
pub fn self_ipi_x2apic(vector: u8) {
    x2apic_write32(X2APIC_SELF_IPI, vector as u32);
}

/// LVT timer configuration in TSC-Deadline mode
pub fn timer_init_tsc_deadline_x2apic(vector: u8) {
    let mode_tsc_deadline = 0b10 << 17;
    x2apic_write32(X2APIC_LVT_TIMER, mode_tsc_deadline | (vector as u32));
}

/// Programs the next TSC deadline (same as in xAPIC)
#[inline]
pub fn timer_set_deadline_x2apic(deadline: u64) {
    // SAFETY: MSR TSC_DEADLINE available if CPUID validated
    unsafe {
        msr::write_msr(msr::MSR_TSC_DEADLINE, deadline);
    }
}
