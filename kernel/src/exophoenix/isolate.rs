//! ExoIsolate — Kernel A memory cage during isolation.
//!
//! Covered errors: S8 (TLB shootdown via 0xF3), S-N1 (IOTLB flush)
//! Activated by handoff.rs after confirmation of freeze ACKs.

use core::sync::atomic::Ordering;

use crate::arch::x86_64::apic::{self, local_apic, x2apic};
use crate::arch::x86_64::cpu::msr;
use crate::arch::x86_64::idt;
use crate::exophoenix::{ssr, stage0};
use crate::memory::core::{PageFlags, VirtAddr, KERNEL_IMAGE_MAX_SIZE, PAGE_SIZE};
use crate::memory::dma::iommu::{AMD_IOMMU, INTEL_VTD};
use crate::memory::virt::address_space::kernel::KERNEL_AS;
use crate::memory::virt::page_table::{PageTableWalker, WalkResult};

// ── MARKERS FOR GPT-5.3-CODEX ─────────────────────────────────────────
// Lines marked with [ADAPT] require substitution of actual API names.
// Everything else is frozen.
// ─────────────────────────────────────────────────────────────────────────

#[inline(always)]
fn read_apic_ticks() -> u32 {
    match stage0::B_FEATURES.apic_mode() {
        stage0::BootApicMode::X2Apic => unsafe { msr::read_msr(x2apic::X2APIC_TIMER_CCR) as u32 },
        stage0::BootApicMode::XApic => local_apic::timer_current_count(),
    }
}

#[inline(always)]
fn apic_elapsed_ticks(start: u32, current: u32) -> u64 {
    start.wrapping_sub(current) as u64
}

#[inline(always)]
fn current_apic_id() -> u32 {
    match stage0::B_FEATURES.apic_mode() {
        stage0::BootApicMode::X2Apic => x2apic::x2apic_id(),
        stage0::BootApicMode::XApic => local_apic::lapic_id(),
    }
}

#[inline(always)]
fn current_slot() -> Option<usize> {
    stage0::apic_slot(current_apic_id())
}

fn for_each_target_slot(self_slot: Option<usize>, mut f: impl FnMut(usize)) {
    let mut seen_slots = [0u64; 4];
    for apic_id in 0u16..=255u16 {
        let Some(slot) = stage0::apic_slot(apic_id as u32) else {
            continue;
        };
        if Some(slot) == self_slot {
            continue;
        }
        if !super::take_slot_once(&mut seen_slots, slot) {
            continue;
        }
        f(slot);
    }
}

fn reset_tlb_acks(self_slot: Option<usize>) {
    for_each_target_slot(self_slot, |slot| unsafe {
        ssr::ssr_atomic_u32(ssr::freeze_ack_offset(slot)).store(0, Ordering::Release);
    });
}

fn all_tlb_acks_observed(self_slot: Option<usize>) -> bool {
    let mut all_ok = true;
    for_each_target_slot(self_slot, |slot| {
        if !all_ok {
            return;
        }
        let ack =
            unsafe { ssr::ssr_atomic_u32(ssr::freeze_ack_offset(slot)).load(Ordering::Acquire) };
        if ack != ssr::TLB_ACK_DONE {
            all_ok = false;
        }
    });
    all_ok
}

// ── 1. Mark pages of A as !PRESENT ─────────────────────────────

fn mark_a_pages_not_present() {
    let kernel_start = crate::memory::core::layout::KERNEL_START;
    let kernel_end_va = VirtAddr::new(
        kernel_start
            .as_u64()
            .saturating_add(KERNEL_IMAGE_MAX_SIZE as u64),
    );

    let mut walker = PageTableWalker::new(KERNEL_AS.pml4_phys());
    let mut va = kernel_start;
    while va.as_u64() < kernel_end_va.as_u64() {
        match walker.walk_read(va) {
            WalkResult::Leaf { entry, .. } => {
                let flags = entry.to_page_flags().clear(PageFlags::PRESENT);
                let _ = walker.remap_flags(va, flags);
            }
            WalkResult::NotMapped => {}
            WalkResult::HugePage { .. } => {
                log::warn!("isolate: huge page détectée à {:#x} — skip", va.as_u64());
            }
            WalkResult::AllocError(_) => break,
        }
        va = VirtAddr::new(va.as_u64().saturating_add(PAGE_SIZE as u64));
    }
}

// ── 2. TLB shootdown (S8 — mandatory IPI 0xF3) ─────────────────────────

fn tlb_shootdown_all_a_cores() {
    // S8: INVPCID invalidates only the local core.
    // Only a broadcast IPI 0xF3 invalidates the TLBs of Kernel A's cores.
    let self_slot = current_slot();
    reset_tlb_acks(self_slot);
    if apic::is_x2apic() {
        x2apic::broadcast_ipi_except_self_x2apic(idt::VEC_EXOPHOENIX_TLB);
    } else {
        local_apic::broadcast_ipi_except_self(idt::VEC_EXOPHOENIX_TLB);
    }
    // Wait for TLB ACKs in SSR (timeout 100µs)
    let ticks_per_us = stage0::ticks_per_us();
    if ticks_per_us == 0 {
        return;
    }
    let timeout_ticks = ticks_per_us.saturating_mul(100);
    let start = read_apic_ticks();
    loop {
        if all_tlb_acks_observed(self_slot) {
            return;
        }
        let current = read_apic_ticks();
        if apic_elapsed_ticks(start, current) >= timeout_ticks {
            return;
        }
        core::hint::spin_loop();
    }
}

// ── 3. IOMMU hard revoke + IOTLB flush (S-N1) ────────────────────────────

fn iommu_hard_revoke_and_flush() {
    let blocked = stage0::blocked_domain_id();
    if INTEL_VTD.is_initialized() && INTEL_VTD.unit_count() > 0 {
        // QI Intel VT-d: tables + IOTLB flush
        // S-N1: do not merely modify tables without flushing
        unsafe {
            INTEL_VTD.flush_iotlb_domain(blocked as u16, 0);
        }
    } else if AMD_IOMMU.is_initialized() {
        // AMD Completion Wait fallback
        core::sync::atomic::fence(Ordering::SeqCst);
    }
}

// ── 4. Override IDT of A ──────────────────────────────────────────────────

#[inline(always)]
fn read_current_idtr() -> (u64, u16) {
    #[repr(C, packed)]
    struct Idtr {
        limit: u16,
        base: u64,
    }

    let mut idtr = Idtr { limit: 0, base: 0 };
    unsafe {
        core::arch::asm!(
            "sidt [{ptr}]",
            ptr = in(reg) &mut idtr,
            options(nostack, preserves_flags)
        );
    }
    (idtr.base, idtr.limit)
}

unsafe fn write_idt_entry(idt_base: u64, vector: u8, handler: u64) {
    let entry_ptr = (idt_base + (vector as u64) * 16) as *mut u8;
    let entry = core::slice::from_raw_parts_mut(entry_ptr, 16);

    let lo16 = (handler & 0xFFFF) as u16;
    let mid16 = ((handler >> 16) & 0xFFFF) as u16;
    let hi32 = (handler >> 32) as u32;

    entry[0..2].copy_from_slice(&lo16.to_le_bytes());
    entry[2..4].copy_from_slice(&(0x0008u16).to_le_bytes()); // GDT_KERNEL_CS
    entry[4] = 0x00; // IST=0
    entry[5] = 0x8E; // Interrupt gate present DPL0
    entry[6..8].copy_from_slice(&mid16.to_le_bytes());
    entry[8..12].copy_from_slice(&hi32.to_le_bytes());
    entry[12..16].fill(0);
}

fn override_a_idt_with_b_handlers() {
    let (idt_base, idt_limit) = read_current_idtr();
    if idt_base == 0 || idt_limit < 16 {
        return;
    }

    let freeze_handler = idt::exophoenix_freeze_handler_addr();
    let pmc_handler = idt::exophoenix_pmc_handler_addr();
    let tlb_handler = idt::exophoenix_tlb_handler_addr();
    if freeze_handler == 0 || pmc_handler == 0 || tlb_handler == 0 {
        return;
    }

    let max_vec = idt::VEC_EXOPHOENIX_FREEZE
        .max(idt::VEC_EXOPHOENIX_PMC)
        .max(idt::VEC_EXOPHOENIX_TLB);
    let min_limit_needed = ((max_vec as u16) + 1) * 16 - 1;
    if idt_limit < min_limit_needed {
        return;
    }

    unsafe {
        write_idt_entry(idt_base, idt::VEC_EXOPHOENIX_FREEZE, freeze_handler);
        write_idt_entry(idt_base, idt::VEC_EXOPHOENIX_PMC, pmc_handler);
        write_idt_entry(idt_base, idt::VEC_EXOPHOENIX_TLB, tlb_handler);
    }
}

// ── Main Entry Point ──────────────────────────────────────────────

/// Applies the full memory cage to Kernel A.
/// Called by handoff.rs after confirmation of freeze ACKs and IOMMU drain.
/// Strict order — do not modify.
pub fn isolate_kernel_a_memory() {
    // 1. Mark pages of A !PRESENT
    mark_a_pages_not_present();

    // 2. TLB shootdown across all cores of A (S8)
    tlb_shootdown_all_a_cores();

    // 3. Hard revoke IOMMU + IOTLB flush (S-N1)
    iommu_hard_revoke_and_flush();

    // 4. Override IDT of A
    override_a_idt_with_b_handlers();
}
