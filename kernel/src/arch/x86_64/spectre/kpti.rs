//! # arch/x86_64/spectre/kpti.rs — Kernel Page-Table Isolation
//!
//! KPTI isolates kernel/user page tables to mitigate Meltdown:
//! - Kernel uses `cr3_kernel` (full access)
//! - Userspace uses `cr3_user` (only mapped syscall/exception stubs)
//!
//! ## Implementation
//! CR3 switching is performed in `switch_asm.s` (between register PUSH/POP).
//! This module exclusively handles detection, global state, and switch helpers.

use crate::arch::x86_64::smp::percpu;
use crate::arch::x86_64::smp::percpu::MAX_CPUS;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ── Global KPTI State ─────────────────────────────────────────────────────────

static KPTI_ENABLED: AtomicBool = AtomicBool::new(false);

/// Returns `true` if KPTI is enabled on this system
pub fn kpti_enabled() -> bool {
    KPTI_ENABLED.load(Ordering::Relaxed)
}

// ── PCID Bits ─────────────────────────────────────────────────────────────────

const CR3_PCID_MASK: u64 = 0xFFF;
const CR3_NO_FLUSH_BIT: u64 = 1u64 << 63;

pub const PCID_KERNEL: u64 = 0;
pub const PCID_USER: u64 = 1;

// ── Per-CPU CR3 — FIX-KPTI-01: replaces the two global AtomicU64s ────────────
//
// BEFORE (BUG):
//   static CURRENT_CR3_KERNEL: AtomicU64 = AtomicU64::new(0); // single one for all
//   static CURRENT_CR3_USER:   AtomicU64 = AtomicU64::new(0); // single one for all
//
// AFTER: one 128-byte slot per CPU → no false sharing, no SMP race.

#[repr(C, align(128))]
struct Cr3Slot {
    kernel: AtomicU64,
    user: AtomicU64,
    _pad: [u8; 112],
}

impl Cr3Slot {
    const fn new() -> Self {
        Self {
            kernel: AtomicU64::new(0),
            user: AtomicU64::new(0),
            _pad: [0u8; 112],
        }
    }
}

unsafe impl Sync for Cr3Slot {}

static CR3_PER_CPU: [Cr3Slot; MAX_CPUS] = [const { Cr3Slot::new() }; MAX_CPUS];

#[inline(always)]
fn current_cpu_slot() -> &'static Cr3Slot {
    let cpu_id = percpu::current_cpu_id() as usize;
    &CR3_PER_CPU[cpu_id.min(MAX_CPUS - 1)]
}

/// Records the pair (cr3_kernel, cr3_user) for the current thread on THIS CPU.
/// Must be called after each context switch (FIX-KPTI-01).
pub fn set_current_cr3(cr3_kernel: u64, cr3_user: u64) {
    if !kpti_enabled() {
        percpu::set_kpti_cr3_slots(0, 0);
        return;
    }
    let slot = current_cpu_slot();
    slot.kernel.store(cr3_kernel, Ordering::Release);
    slot.user.store(cr3_user, Ordering::Release);
    percpu::set_kpti_cr3_slots(cr3_kernel & !CR3_PCID_MASK, cr3_user & !CR3_PCID_MASK);

    // FIX-KPTI-SHADOW-SYNC: the KPTI user shadow is SHARED per CPU; its user
    // half must reflect the current process at each switch, otherwise a page
    // demand-paged for process A disappears as soon as B executes then returns
    // control to A → #PF loop (bootstrap init after fork).
    //
    // Target = the ACTUAL per-CPU shadow used by entry trampoline
    // (kpti_split::user_cr3_for_cpu); TCB's `cr3_user` can be 0 (PID 1).
    const PML4_PHYS_MASK: u64 = 0x000F_FFFF_FFFF_F000;
    let k = cr3_kernel & PML4_PHYS_MASK;
    let cpu_id = percpu::current_cpu_id() as usize;
    let shadow = crate::memory::virt::page_table::kpti_split::user_cr3_for_cpu(cpu_id).unwrap_or(0)
        & PML4_PHYS_MASK;
    let candidates = [cr3_user & PML4_PHYS_MASK, shadow];
    let mut synced: u64 = 0;
    for u in candidates {
        if k != 0 && u != 0 && k != u && u != synced {
            synced = u;
            // SAFETY: k/u reference valid PML4s (process / per-CPU shadow).
            unsafe {
                crate::memory::virt::page_table::kpti_split::sync_user_region_to_shadow(
                    crate::memory::core::PhysAddr::new(k),
                    crate::memory::core::PhysAddr::new(u),
                );
            }
        }
    }
}

/// Returns user CR3 (shadow KPTI) currently published for THIS CPU, i.e.
/// the one on which current thread actually executes in Ring 3.
///
/// Used as source of truth by fault synchronization when TCB
/// does not (yet) carry its `kpti_user_cr3` — e.g. PID 1 created by handoff.
#[inline]
pub fn current_user_cr3() -> u64 {
    current_cpu_slot().user.load(Ordering::Acquire) & !CR3_PCID_MASK
}

// ── Kernel → User CR3 Switch ─────────────────────────────────────────────────

/// Switches to user page tables (to execute before IRETQ to Ring 3)
///
/// # Safety
/// Executable strictly from Ring 0 right before returning to userspace.
#[inline]
pub unsafe fn kpti_switch_to_user() {
    if !KPTI_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let slot = current_cpu_slot();
    let cr3_user = slot.user.load(Ordering::Acquire);
    if cr3_user == 0 {
        log::warn!("KPTI actif mais aucun CR3 user n'est publie pour ce CPU");
        return;
    }
    let features = super::super::cpu::features::cpu_features();
    let cr3 = if features.has_pcid() {
        cr3_user | PCID_USER | CR3_NO_FLUSH_BIT
    } else {
        cr3_user & !CR3_PCID_MASK
    };
    core::arch::asm!("mov cr3, {}", in(reg) cr3, options(nostack, nomem));
}

/// Switches to kernel page tables (on return to Ring 0)
///
/// # Safety
/// Executable strictly from Ring 0 upon entering kernel.
#[inline]
pub unsafe fn kpti_switch_to_kernel() {
    if !KPTI_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let slot = current_cpu_slot();
    let cr3_kernel = slot.kernel.load(Ordering::Acquire);
    if cr3_kernel == 0 {
        log::warn!("KPTI actif mais aucun CR3 kernel n'est publie pour ce CPU");
        return;
    }
    let features = super::super::cpu::features::cpu_features();
    let cr3 = if features.has_pcid() {
        cr3_kernel | PCID_KERNEL | CR3_NO_FLUSH_BIT
    } else {
        cr3_kernel & !CR3_PCID_MASK
    };
    core::arch::asm!("mov cr3, {}", in(reg) cr3, options(nostack, nomem));
}

// ── Initialization ────────────────────────────────────────────────────────────

/// Enables KPTI (called from `apply_mitigations_bsp()`)
pub fn init_kpti() {
    let features = super::super::cpu::features::cpu_features();

    // Enable SMEP if available (prevents kernel from executing user code)
    if features.has_smep() {
        let mut cr4: u64;
        // SAFETY: read CR4
        unsafe {
            core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nostack, nomem));
        }
        cr4 |= 1 << 20; // SMEP
                        // SAFETY: write CR4 — enable SMEP
        unsafe {
            core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nostack, nomem));
        }
    }

    // Enable SMAP if available (prevents kernel from accessing user data without STAC/CLAC)
    if features.has_smap() {
        let mut cr4: u64;
        // SAFETY: read CR4
        unsafe {
            core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nostack, nomem));
        }
        cr4 |= 1 << 21; // SMAP
                        // SAFETY: write CR4 — enable SMAP
        unsafe {
            core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nostack, nomem));
        }
    }

    let cpu_id = crate::arch::x86_64::smp::percpu::current_cpu_id() as usize;
    let kernel_pml4_phys = crate::memory::virt::page_table::read_cr3();
    let trampoline_phys =
        crate::memory::core::PhysAddr::new(crate::arch::x86_64::smp::init::TRAMPOLINE_PHYS);

    let user_shadow = unsafe {
        crate::memory::virt::page_table::kpti_split::build_user_shadow_pml4(
            kernel_pml4_phys,
            cpu_id,
            trampoline_phys,
            crate::arch::x86_64::smp::percpu::read_kernel_rsp(),
        )
    };

    match user_shadow {
        Ok(user_pml4_phys) => {
            unsafe {
                crate::memory::virt::page_table::kpti_split::KPTI.register_cpu(
                    cpu_id,
                    kernel_pml4_phys,
                    user_pml4_phys,
                    trampoline_phys,
                );
            }
            crate::memory::virt::page_table::kpti_split::KPTI.enable();
            KPTI_ENABLED.store(true, Ordering::Release);
            // FIX-KPTI-01: initialize per-CPU slot upon activation.
            set_current_cr3(kernel_pml4_phys.as_u64(), user_pml4_phys.as_u64());
        }
        Err(_) => {
            log::warn!(
                "KPTI: impossible d'allouer la user_pml4 shadow sur CPU {}",
                cpu_id
            );
        }
    }
}
