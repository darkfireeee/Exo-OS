//! ExoPhoenix handlers for reserved vectors.
//! Absolute rule: lock-free, zero allocation, atomics + MSR/CR3 only.

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::Ordering;

use crate::arch::x86_64::apic::x2apic;
use crate::arch::x86_64::cpu::msr;
use crate::scheduler::core::task::ThreadControlBlock;

use super::ssr;
use super::stage0;

const APICBASE_ADDR_MASK: u64 = 0xFFFF_FFFF_F000;
const LAPIC_ID_REG_OFFSET: usize = 0x20;
const LAPIC_ACK_REG_OFFSET: usize = 0xB0;

#[inline(always)]
fn xapic_mmio_base() -> usize {
    // SAFETY: reading an architectural MSR in Ring 0.
    let apic_base = unsafe { msr::read_msr(msr::MSR_IA32_APIC_BASE) } & APICBASE_ADDR_MASK;
    apic_base as usize
}

#[inline(always)]
fn current_apic_id() -> u32 {
    match stage0::B_FEATURES.apic_mode() {
        stage0::BootApicMode::X2Apic => {
            // SAFETY: x2APIC active and MSR X2APIC_ID readable.
            unsafe { msr::read_msr(x2apic::X2APIC_ID) as u32 }
        }
        stage0::BootApicMode::XApic => {
            let lapic_id_ptr = (xapic_mmio_base() + LAPIC_ID_REG_OFFSET) as *const u32;
            // SAFETY: LAPIC MMIO active in xAPIC mode.
            unsafe { read_volatile(lapic_id_ptr) >> 24 }
        }
    }
}

#[inline(always)]
fn current_slot() -> Option<usize> {
    stage0::apic_slot(current_apic_id())
}

#[inline(always)]
fn apic_ack() {
    match stage0::B_FEATURES.apic_mode() {
        stage0::BootApicMode::X2Apic => {
            // SAFETY: x2APIC active; EOI via dedicated MSR.
            unsafe {
                msr::write_msr(x2apic::X2APIC_EOI, 0);
            }
        }
        stage0::BootApicMode::XApic => {
            let lapic_eoi_ptr = (xapic_mmio_base() + LAPIC_ACK_REG_OFFSET) as *mut u32;
            // SAFETY: LAPIC MMIO active in xAPIC mode.
            unsafe {
                write_volatile(lapic_eoi_ptr, 0);
            }
        }
    }
}

unsafe fn save_current_fpu_before_freeze_ack() {
    let tcb_raw = unsafe { crate::arch::x86_64::smp::percpu::try_read_current_tcb() }.unwrap_or(0);
    if tcb_raw == 0 {
        return;
    }

    let tcb = &mut *(tcb_raw as *mut ThreadControlBlock);
    if !tcb.fpu_loaded() {
        return;
    }

    if crate::scheduler::fpu::lazy::cr0_ts_is_set() {
        tcb.set_fpu_loaded(false);
        return;
    }

    crate::scheduler::fpu::xsave_current(tcb);
    crate::scheduler::fpu::lazy::cr0_set_ts();
}

/// 0xF1 — Cooperative freeze.
///
/// - CLI (maskable IRQs off; NMI remains possible by x86 design)
/// - Potential XSAVE before ACK (CORR-15 / TLA S3)
/// - ACK FREEZE with Release ordering
/// - spin until Kernel B resumption
pub unsafe fn handle_freeze_ipi() {
    // SAFETY: privileged instruction in ring0 IRQ handler context.
    unsafe {
        core::arch::asm!("cli", options(nostack, nomem));
    }

    save_current_fpu_before_freeze_ack();

    if let Some(slot) = current_slot() {
        // SAFETY: fixed/reserved physical SSR; offset bounded via slot map.
        unsafe {
            ssr::ssr_atomic_u32(ssr::freeze_ack_offset(slot))
                .store(ssr::FREEZE_ACK_DONE, Ordering::Release);
            core::arch::asm!("sfence", options(nostack, preserves_flags));
        }
    }

    loop {
        let handoff = unsafe { ssr::ssr_atomic(ssr::SSR_HANDOFF_FLAG).load(Ordering::Acquire) };
        if handoff == ssr::HANDOFF_B_ACTIVE || handoff == ssr::HANDOFF_NORMAL {
            break;
        }

        // SAFETY: intentional freeze loop.
        unsafe {
            core::arch::asm!("pause", options(nostack, nomem));
        }
    }

    apic_ack();
}

/// 0xF2 — PMC snapshot (heuristic, never single source of truth).
pub unsafe fn handle_pmc_snapshot_ipi() {
    if !stage0::B_FEATURES.pmc_available() {
        apic_ack();
        return;
    }

    let Some(slot) = current_slot() else {
        apic_ack();
        return;
    };

    let base = ssr::SSR_BASE as usize + ssr::pmc_snapshot_offset(slot);

    for i in 0..4u32 {
        // SAFETY: MSR access conditioned by pmc_available.
        let evtsel = unsafe { msr::read_msr(msr::MSR_IA32_PERFEVTSEL0 + i) };
        // SAFETY: ditto, sequential PMC bank IA32_PMC0..3.
        let ctr = unsafe { msr::read_msr(msr::MSR_IA32_PMC0 + i) };

        // SAFETY: volatile writes in SSR dedicated to current slot.
        unsafe {
            core::ptr::write_volatile((base + (i as usize) * 16) as *mut u64, evtsel);
            core::ptr::write_volatile((base + (i as usize) * 16 + 8) as *mut u64, ctr);
        }
    }

    apic_ack();
}

/// 0xF3 — Local global TLB flush (reload CR3) + ACK.
pub unsafe fn handle_tlb_flush_ipi() {
    // SAFETY: privileged instruction in ring0 IRQ handler context.
    unsafe {
        core::arch::asm!("cli", options(nostack, nomem));
    }

    // SAFETY: reload current CR3 to invalidate local TLB.
    unsafe {
        core::arch::asm!(
            "mov {tmp}, cr3",
            "mov cr3, {tmp}",
            tmp = out(reg) _,
            options(nostack)
        );
    }

    if let Some(slot) = current_slot() {
        // SAFETY: fixed/reserved physical SSR; offset bounded via slot map.
        unsafe {
            ssr::ssr_atomic_u32(ssr::freeze_ack_offset(slot))
                .store(ssr::TLB_ACK_DONE, Ordering::Release);
        }
    }

    apic_ack();
}
