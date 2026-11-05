// kernel/src/memory/protection/nx.rs
//
// NX (No-Execute) bit management — EFER.NXE activation + strict
// enforcement per memory zone.
//
// References: Intel SDM Vol.3A § 4.6 — "Access Rights"; AMD APM Vol.2 § 5.6.
//
// Exo-OS architecture rules:
//   LAYER 0 — no dependency on scheduler/, process/, ipc/, fs/.
//   All MSR accesses go through inline `asm!`.
//   No std; spin = "0.9" only for locks.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::memory::core::constants::PAGE_SIZE;
use crate::memory::core::layout::{
    KERNEL_HEAP_START, PHYS_MAP_BASE, PHYS_MAP_END, VMALLOC_BASE, VMALLOC_END,
};
use crate::memory::core::types::PageFlags;

// ─────────────────────────────────────────────────────────────────────────────
// Internal constants
// ─────────────────────────────────────────────────────────────────────────────

/// Bit 11 of MSR_EFER = NXE (No-Execute Enable).
const EFER_NXE_BIT: u64 = 1 << 11;
/// MSR EFER number.
const MSR_EFER: u32 = 0xC000_0080;
/// Bit 63 of a page table entry = XD/NX.
pub const PAGE_TABLE_NX_BIT: u64 = 1u64 << 63;

// ─────────────────────────────────────────────────────────────────────────────
// NX Statistics
// ─────────────────────────────────────────────────────────────────────────────

/// Operational counters for NX subsystem.
#[repr(C)]
pub struct NxStats {
    /// Number of times EFER.NXE was enabled (must remain 1 after init).
    pub efer_nxe_enable_count: AtomicU64,
    /// Captured NX violations (#PF with I/D bit = 1, execute = 1).
    pub violation_count: AtomicU64,
    /// Pages marked NX during initialization.
    pub pages_marked_nx: AtomicU64,
    /// Pages whose NX flag was explicitly cleared (text segments).
    pub pages_cleared_nx: AtomicU64,
    /// Number of calls to `nx_enforce_region`.
    pub region_enforce_calls: AtomicU64,
    /// Attempts to toggle NX when EFER.NXE is already active.
    pub redundant_enable: AtomicU64,
}

impl NxStats {
    const fn new() -> Self {
        Self {
            efer_nxe_enable_count: AtomicU64::new(0),
            violation_count: AtomicU64::new(0),
            pages_marked_nx: AtomicU64::new(0),
            pages_cleared_nx: AtomicU64::new(0),
            region_enforce_calls: AtomicU64::new(0),
            redundant_enable: AtomicU64::new(0),
        }
    }
}

// SAFETY: all fields are AtomicU64 -> safe to share across CPUs.
unsafe impl Sync for NxStats {}

pub static NX_STATS: NxStats = NxStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// Subsystem state
// ─────────────────────────────────────────────────────────────────────────────

/// `true` once EFER.NXE is enabled on the BSP.
static NX_ENABLED: AtomicBool = AtomicBool::new(false);

// ─────────────────────────────────────────────────────────────────────────────
// Regional NX policy
// ─────────────────────────────────────────────────────────────────────────────

/// NX policy for a virtual address range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NxPolicy {
    /// Range is executable (kernel code / module).
    Executable,
    /// Range is non-executable (data, stack, heap, physmap, vmalloc data).
    NonExecutable,
    /// Range is non-present / not affected.
    Absent,
}

/// Static regional rule: [start_virt, end_virt) -> NxPolicy.
#[derive(Debug, Clone, Copy)]
pub struct NxRegionRule {
    pub start: u64,
    pub end: u64,
    pub policy: NxPolicy,
}

/// Statically compiled NX policy table (.rodata).
/// RULE IA-KERNEL-01: .rodata table, no runtime generation.
static NX_REGION_RULES: [NxRegionRule; 6] = [
    // Physmap — mapped data, never executable.
    NxRegionRule {
        start: PHYS_MAP_BASE.as_u64(),
        end: PHYS_MAP_END.as_u64(),
        policy: NxPolicy::NonExecutable,
    },
    // Vmalloc — kernel dynamic allocations, data.
    NxRegionRule {
        start: VMALLOC_BASE.as_u64(),
        end: VMALLOC_END.as_u64(),
        policy: NxPolicy::NonExecutable,
    },
    // Kernel heap — never executable.
    NxRegionRule {
        start: KERNEL_HEAP_START.as_u64(),
        end: KERNEL_HEAP_START.as_u64() + 0x0000_0100_0000_0000,
        policy: NxPolicy::NonExecutable,
    },
    // CPU stacks (fixed 64 KiB per CPU, 256 CPUs max = 16 MiB total).
    NxRegionRule {
        start: 0xFFFF_FF80_0000_0000,
        end: 0xFFFF_FF80_0100_0000,
        policy: NxPolicy::NonExecutable,
    },
    // Kernel code (.text/.init segments).
    NxRegionRule {
        start: 0xFFFF_FFFF_8000_0000,
        end: 0xFFFF_FFFF_A000_0000,
        policy: NxPolicy::Executable,
    },
    // Kernel data (.data/.bss/.rodata).
    NxRegionRule {
        start: 0xFFFF_FFFF_A000_0000,
        end: 0xFFFF_FFFF_C000_0000,
        policy: NxPolicy::NonExecutable,
    },
];

// ─────────────────────────────────────────────────────────────────────────────
// Read / write MSR EFER via inline asm!
// ─────────────────────────────────────────────────────────────────────────────

/// Reads the MSR identified by `msr`.
///
/// # Safety
/// Must be executed in CPL 0 (ring 0).
#[inline(always)]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    ((hi as u64) << 32) | (lo as u64)
}

/// Writes `val` into the MSR identified by `msr`.
///
/// # Safety
/// Must be executed in CPL 0. Modifying EFER incorrectly can render the CPU unusable.
#[inline(always)]
unsafe fn wrmsr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") lo,
        in("edx") hi,
        options(nostack, nomem),
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Enables `EFER.NXE` on current CPU.
///
/// Must be called on **each** CPU (BSP then APs) before activating
/// NX page tables. Idempotent: if already enabled, increments `redundant_enable`.
///
/// # Safety
/// Must be executed in CPL 0 with interrupts disabled if page table
/// switch follows immediately.
pub unsafe fn enable_nx() {
    let efer = rdmsr(MSR_EFER);
    if efer & EFER_NXE_BIT != 0 {
        NX_STATS.redundant_enable.fetch_add(1, Ordering::Relaxed);
        return;
    }
    wrmsr(MSR_EFER, efer | EFER_NXE_BIT);
    NX_STATS
        .efer_nxe_enable_count
        .fetch_add(1, Ordering::Relaxed);
    NX_ENABLED.store(true, Ordering::Release);
}

/// Verifies that EFER.NXE is active on current CPU.
///
/// # Safety
/// CPL 0.
#[inline]
pub unsafe fn is_nx_active() -> bool {
    rdmsr(MSR_EFER) & EFER_NXE_BIT != 0
}

/// Returns `true` if the NX subsystem has been initialized.
#[inline]
pub fn nx_enabled() -> bool {
    NX_ENABLED.load(Ordering::Acquire)
}

/// Determines the NX policy of a virtual address according to regional table.
#[inline]
pub fn nx_policy_for(virt: u64) -> NxPolicy {
    for rule in &NX_REGION_RULES {
        if virt >= rule.start && virt < rule.end {
            return rule.policy;
        }
    }
    // Default: data — non-executable (secure by default).
    NxPolicy::NonExecutable
}

/// Returns appropriate `PageFlags` for virtual address `virt`:
/// - NX enabled unless the region is executable and NX is active.
#[inline]
pub fn nx_page_flags(virt: u64, flags: PageFlags) -> PageFlags {
    if !nx_enabled() {
        return flags;
    }
    match nx_policy_for(virt) {
        NxPolicy::NonExecutable => flags | PageFlags::NO_EXECUTE,
        NxPolicy::Executable => flags & !PageFlags::NO_EXECUTE,
        NxPolicy::Absent => flags,
    }
}

/// Enforces NX policy on a range of pages during page-table walk.
///
/// This function walks [`count`] pages starting from `base_virt` and forces
/// the NX bit according to `NX_REGION_RULES`. The caller provides a closure that
/// receives the virtual address and returns `Option<*mut u64>` pointing to the
/// PTE entry (level 1) in memory.
///
/// # Safety
/// - PTEs must be valid and in non-paged memory.
/// - Must be called with EFER.NXE active.
pub unsafe fn nx_enforce_region<F>(base_virt: u64, count: usize, pte_resolver: F)
where
    F: Fn(u64) -> Option<*mut u64>,
{
    NX_STATS
        .region_enforce_calls
        .fetch_add(1, Ordering::Relaxed);

    for i in 0..count as u64 {
        let vaddr = base_virt + i * PAGE_SIZE as u64;
        if let Some(pte_ptr) = pte_resolver(vaddr) {
            let pte = pte_ptr.read_volatile();
            // If entry is not present, skip.
            if pte & 1 == 0 {
                continue;
            }
            let new_pte = match nx_policy_for(vaddr) {
                NxPolicy::NonExecutable => {
                    NX_STATS.pages_marked_nx.fetch_add(1, Ordering::Relaxed);
                    pte | PAGE_TABLE_NX_BIT
                }
                NxPolicy::Executable => {
                    NX_STATS.pages_cleared_nx.fetch_add(1, Ordering::Relaxed);
                    pte & !PAGE_TABLE_NX_BIT
                }
                NxPolicy::Absent => pte,
            };
            if new_pte != pte {
                pte_ptr.write_volatile(new_pte);
                // Invalidate TLB for this page.
                core::arch::asm!(
                    "invlpg [{addr}]",
                    addr = in(reg) vaddr as usize,
                    options(nostack, nomem, preserves_flags),
                );
            }
        }
    }
}

/// Handler called by fault handler (#PF) when an NX violation is detected
/// (fault_flags indicates execute access on NX page).
///
/// Increments `violation_count` and returns `false` to signal main handler
/// that it must trigger a kernel panic or terminate process.
#[inline]
pub fn nx_handle_violation(fault_virt: u64, fault_ip: u64) -> bool {
    NX_STATS.violation_count.fetch_add(1, Ordering::Relaxed);
    // Log (via serial / ring-buffer) without scheduler call.
    // Minimal format for pre-console ring 0.
    let _ = fault_virt;
    let _ = fault_ip;
    false // non-recoverable -> caller must panic!
}

/// NX subsystem initialization.
/// Must be first function called on BSP before mapping final page tables.
///
/// # Safety
/// CPL 0, interrupts disabled recommended.
pub unsafe fn init() {
    enable_nx();
    // Post-activation consistency check.
    debug_assert!(is_nx_active(), "NX: EFER.NXE non actif après enable_nx()");
}
