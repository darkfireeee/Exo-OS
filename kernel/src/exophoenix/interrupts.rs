//! Handlers ExoPhoenix pour les vecteurs réservés.
//! Règle absolue : lock-free, aucune allocation, uniquement atomics + MSR/CR3.

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
            // SAFETY: x2APIC actif ; EOI via MSR dédié.
            unsafe {
                msr::write_msr(x2apic::X2APIC_EOI, 0);
            }
        }
        stage0::BootApicMode::XApic => {
            let lapic_eoi_ptr = (xapic_mmio_base() + LAPIC_ACK_REG_OFFSET) as *mut u32;
            // SAFETY: LAPIC MMIO actif en mode xAPIC.
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

/// 0xF1 — Freeze coopératif.
///
/// - CLI (IRQ maskables off ; NMI reste possible par design x86)
/// - XSAVE éventuel avant ACK (CORR-15 / TLA S3)
/// - ACK FREEZE en Release
/// - spin jusqu'à reprise Kernel B
pub unsafe fn handle_freeze_ipi() {
    // SAFETY: instruction privilégiée en contexte handler IRQ ring0.
    unsafe {
        core::arch::asm!("cli", options(nostack, nomem));
    }

    save_current_fpu_before_freeze_ack();

    if let Some(slot) = current_slot() {
        // SAFETY: SSR physique fixée/réservée ; offset borné via slot map.
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

        // SAFETY: boucle de gel volontaire.
        unsafe {
            core::arch::asm!("pause", options(nostack, nomem));
        }
    }

    apic_ack();
}

/// 0xF2 — PMC snapshot (heuristique, jamais source unique de vérité).
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
        // SAFETY: accès MSR conditionné par pmc_available.
        let evtsel = unsafe { msr::read_msr(msr::MSR_IA32_PERFEVTSEL0 + i) };
        // SAFETY: idem, PMC bank séquentielle IA32_PMC0..3.
        let ctr = unsafe { msr::read_msr(msr::MSR_IA32_PMC0 + i) };

        // SAFETY: écritures volatiles dans SSR dédiée au slot courant.
        unsafe {
            core::ptr::write_volatile((base + (i as usize) * 16) as *mut u64, evtsel);
            core::ptr::write_volatile((base + (i as usize) * 16 + 8) as *mut u64, ctr);
        }
    }

    apic_ack();
}

/// 0xF3 — TLB flush global local (reload CR3) + ACK.
pub unsafe fn handle_tlb_flush_ipi() {
    // SAFETY: instruction privilégiée en contexte handler IRQ ring0.
    unsafe {
        core::arch::asm!("cli", options(nostack, nomem));
    }

    // SAFETY: rechargement du CR3 courant pour invalider le TLB local.
    unsafe {
        core::arch::asm!(
            "mov {tmp}, cr3",
            "mov cr3, {tmp}",
            tmp = out(reg) _,
            options(nostack)
        );
    }

    if let Some(slot) = current_slot() {
        // SAFETY: SSR physique fixée/réservée ; offset borné via slot map.
        unsafe {
            ssr::ssr_atomic_u32(ssr::freeze_ack_offset(slot))
                .store(ssr::TLB_ACK_DONE, Ordering::Release);
        }
    }

    apic_ack();
}
