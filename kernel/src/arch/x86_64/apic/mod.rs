//! # arch/x86_64/apic — Advanced Programmable Interrupt Controller
//!
//! Complete management of LAPIC (Local APIC) and I/O APIC for Exo-OS.
//!
//! ## Submodules
//! - `local_apic`: LAPIC MMIO + x2APIC MSR
//! - `io_apic`: I/O APIC MMIO, redirection table
//! - `x2apic`: detection + switch to x2APIC
//! - `ipi`: sending IPIs to other CPUs

pub mod io_apic;
pub mod ipi;
pub mod local_apic;
pub mod x2apic;

pub use ipi::{
    broadcast_ipi_panic, send_ipi_cpu_hotplug, send_ipi_reschedule, send_ipi_tlb_shootdown,
    send_ipi_wakeup,
};
pub use local_apic::{init_local_apic, lapic_id, ApicMode};

use core::sync::atomic::{AtomicU8, Ordering};

/// Current APIC mode of the system
static APIC_MODE: AtomicU8 = AtomicU8::new(local_apic::ApicMode::XApic as u8);

#[inline(always)]
fn detect_apic_mode() -> ApicMode {
    let features = super::cpu::features::cpu_features();
    // SAFETY: IA32_APIC_BASE MSR is readable in ring 0.
    let apic_base = unsafe { super::cpu::msr::read_msr(super::cpu::msr::MSR_IA32_APIC_BASE) };
    let x2apic_enabled = (apic_base & (1 << 10)) != 0;

    if features.has_x2apic() && x2apic_enabled {
        ApicMode::X2Apic
    } else {
        ApicMode::XApic
    }
}

/// Initializes the entire APIC system (BSP only — called from boot/early_init)
///
/// Sequence:
/// 1. Detect x2APIC support (CPUID)
/// 2. Enable LAPIC (x2APIC or xAPIC MMIO)
/// 3. Configure spurious vector
/// 4. Set up LAPIC timer
pub fn init_apic_system() {
    let features = super::cpu::features::cpu_features();

    match detect_apic_mode() {
        ApicMode::X2Apic => {
            // Already in x2APIC (firmware): normalize LVT state.
            x2apic::mask_all_lvt_x2apic();
            APIC_MODE.store(ApicMode::X2Apic as u8, Ordering::Release);
        }
        ApicMode::XApic => {
            // init_local_apic(): enables xAPIC + masks ALL LVTs (LINT0, THERMAL, PERF, CMCI,
            // ERROR). Without this, QEMU BIOS may leave LINT0 with unmasked vector 0x8E:
            // when PIC sends an IRQ, LINT0 delivers vector 0x8E → missing IDT[0x8E] → #GP.
            local_apic::init_local_apic();
            APIC_MODE.store(ApicMode::XApic as u8, Ordering::Release);
        }
    }

    // Configure spurious vector (0xFF) + soft-enable LAPIC
    local_apic::set_spurious_vector(super::idt::VEC_SPURIOUS);

    // Timer: use TSC-Deadline if available, otherwise one-shot
    if features.has_tsc_deadline() {
        local_apic::timer_init_tsc_deadline(super::idt::VEC_IRQ_TIMER);
    } else {
        local_apic::timer_init_oneshot(super::idt::VEC_IRQ_TIMER);
    }
}

/// Initializes current AP's LAPIC (called from smp/init)
pub fn init_ap_local_apic() {
    match apic_mode() {
        ApicMode::X2Apic => {
            // AP already in x2APIC if global mode is x2APIC.
            x2apic::mask_all_lvt_x2apic();
        }
        ApicMode::XApic => {
            local_apic::enable_xapic();
        }
    }
    local_apic::set_spurious_vector(super::idt::VEC_SPURIOUS);
}

#[inline(always)]
pub fn apic_mode() -> ApicMode {
    match APIC_MODE.load(Ordering::Acquire) {
        x if x == ApicMode::X2Apic as u8 => ApicMode::X2Apic,
        _ => ApicMode::XApic,
    }
}

/// Returns `true` if x2APIC is active
#[inline(always)]
pub fn is_x2apic() -> bool {
    apic_mode() == ApicMode::X2Apic
}

/// EOI with automatic xAPIC/x2APIC dispatch
#[inline(always)]
pub fn eoi() {
    if is_x2apic() {
        x2apic::eoi_x2apic();
    } else {
        local_apic::eoi();
    }
}
