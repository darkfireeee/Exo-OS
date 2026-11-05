// kernel/src/memory/physical/numa/migration.rs
//
// NUMA page migration — moves frames between nodes to improve
// access locality.
//
// Algorithm:
//   1. Allocate a destination frame on target node.
//   2. Copy contents from source frame (via physmap).
//   3. Update PTE of affected VMA (atomic TLB shootdown).
//   4. Free source frame.
//
// Caller provides address context (page table root) via trait.
// No direct call to scheduler (layer 0).
//
// Layer 0 — no scheduler/process/ipc/fs dependencies.

use core::sync::atomic::{AtomicU64, Ordering};

use super::node::NUMA_NODES;
use crate::memory::core::address::phys_to_virt;
use crate::memory::core::constants::PAGE_SIZE;
use crate::memory::core::types::{AllocFlags, Frame, PhysAddr};
use crate::memory::physical::allocator::buddy::free_pages;

// ─────────────────────────────────────────────────────────────────────────────
// Statistics
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct MigrationStats {
    pub migrations_attempted: AtomicU64,
    pub migrations_succeeded: AtomicU64,
    pub migrations_failed: AtomicU64,
    /// Number of pages moved between distinct nodes.
    pub pages_moved: AtomicU64,
    /// Optimizations: page already on target node.
    pub already_local: AtomicU64,
    /// Allocation failures on target node.
    pub alloc_failures: AtomicU64,
}

impl MigrationStats {
    const fn new() -> Self {
        Self {
            migrations_attempted: AtomicU64::new(0),
            migrations_succeeded: AtomicU64::new(0),
            migrations_failed: AtomicU64::new(0),
            pages_moved: AtomicU64::new(0),
            already_local: AtomicU64::new(0),
            alloc_failures: AtomicU64::new(0),
        }
    }
}

unsafe impl Sync for MigrationStats {}
pub static MIGRATION_STATS: MigrationStats = MigrationStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// Page-table injection trait — avoids dependency on virtual/
// ─────────────────────────────────────────────────────────────────────────────

/// Trait implemented by `virtual::address_space::user::UserAddressSpace`.
/// memory/ defines the interface, virtual/ implements it.
pub trait MigrationPageTableOps {
    /// Looks up and returns current PTE for `virt_addr` in this context.
    /// Returns `None` if not mapped.
    fn get_pte(&self, virt_addr: u64) -> Option<u64>;

    /// Atomically replaces PTE for `virt_addr` with `new_pte`.
    /// Returns `Ok(old_pte)` or `Err(())` if unable.
    #[allow(clippy::result_unit_err)]
    fn swap_pte(&self, virt_addr: u64, new_pte: u64) -> Result<u64, ()>;

    /// TLB shootdown port for this address space.
    fn flush_tlb(&self, virt_addr: u64);
}

// ─────────────────────────────────────────────────────────────────────────────
// Migration result
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrateResult {
    /// Migration successful — `new_frame` is the destination frame.
    Success { new_frame: Frame },
    /// Page already on target node.
    AlreadyLocal,
    /// Allocation failed on target node.
    AllocFailed,
    /// Unable to update PTE.
    PteUpdateFailed,
    /// Invalid arguments.
    InvalidArgs,
}

// ─────────────────────────────────────────────────────────────────────────────
// Frame copy via physmap
// ─────────────────────────────────────────────────────────────────────────────

/// Copies exactly one frame (PAGE_SIZE bytes) from `src_frame` to `dst_frame`
/// via the kernel physmap.
///
/// # Safety
/// Both frames must be valid and unshared (no active aliases).
unsafe fn copy_frame(src: Frame, dst: Frame) {
    let src_virt = phys_to_virt(PhysAddr::new(src.start_address().as_u64())).as_ptr::<u8>();
    let dst_virt = phys_to_virt(PhysAddr::new(dst.start_address().as_u64())).as_mut_ptr::<u8>();
    core::ptr::copy_nonoverlapping(src_virt, dst_virt, PAGE_SIZE);
}

// ─────────────────────────────────────────────────────────────────────────────
// Frame node
// ─────────────────────────────────────────────────────────────────────────────

/// Returns the NUMA node owner of `frame`.
#[inline]
pub fn frame_node(frame: Frame) -> u32 {
    NUMA_NODES.node_for_phys(frame.start_address().as_u64())
}

// ─────────────────────────────────────────────────────────────────────────────
// migrate_page — moves a frame to `target_node`
// ─────────────────────────────────────────────────────────────────────────────

/// Migrates contents of `src_frame` to a frame allocated on `target_node`.
///
/// If `pt_ops` is `Some`, updates PTE corresponding to `virt_addr` in
/// provided address space and flushes TLB.
///
/// # Safety
/// - Source frame must not be accessed during copy.
/// - PTE must be locked / protected by caller.
pub unsafe fn migrate_page<P: MigrationPageTableOps>(
    src_frame: Frame,
    target_node: u32,
    virt_addr: Option<u64>,
    pt_ops: Option<&P>,
) -> MigrateResult {
    MIGRATION_STATS
        .migrations_attempted
        .fetch_add(1, Ordering::Relaxed);

    // Check if already local.
    let src_node = frame_node(src_frame);
    if src_node == target_node {
        MIGRATION_STATS
            .already_local
            .fetch_add(1, Ordering::Relaxed);
        return MigrateResult::AlreadyLocal;
    }

    // Allocate destination frame on target_node.
    let dst_frame = match alloc_pages_on_node(target_node, 0, AllocFlags::NONE) {
        Some(f) => f,
        None => {
            MIGRATION_STATS
                .alloc_failures
                .fetch_add(1, Ordering::Relaxed);
            MIGRATION_STATS
                .migrations_failed
                .fetch_add(1, Ordering::Relaxed);
            return MigrateResult::AllocFailed;
        }
    };

    // Copy content.
    copy_frame(src_frame, dst_frame);

    // Update PTE if context provided.
    if let (Some(virt), Some(ops)) = (virt_addr, pt_ops) {
        // Build new PTE with same flags as old.
        if let Some(old_pte) = ops.get_pte(virt) {
            // Flags mask: preserve all bits except pfn.
            const PFN_MASK: u64 = !0x0FFF_FFFF_FFFF_F000; // bits 11:0 + 63:48
            let flags = old_pte & PFN_MASK;
            let new_pfn = (dst_frame.start_address().as_u64() >> 12) << 12;
            let new_pte = new_pfn | flags;

            if ops.swap_pte(virt, new_pte).is_err() {
                // Rollback: free destination frame.
                let _ = free_pages(dst_frame, 0);
                MIGRATION_STATS
                    .migrations_failed
                    .fetch_add(1, Ordering::Relaxed);
                return MigrateResult::PteUpdateFailed;
            }
            ops.flush_tlb(virt);
        }
    }

    // Free source frame.
    let _ = free_pages(src_frame, 0);

    // Update node stats.
    if let Some(src_n) = NUMA_NODES.get(src_node) {
        src_n.stats.migrated_out.fetch_add(1, Ordering::Relaxed);
        src_n.stats.record_free();
    }
    if let Some(dst_n) = NUMA_NODES.get(target_node) {
        dst_n.stats.migrated_in.fetch_add(1, Ordering::Relaxed);
        dst_n.stats.record_alloc_local();
    }

    MIGRATION_STATS
        .migrations_succeeded
        .fetch_add(1, Ordering::Relaxed);
    MIGRATION_STATS.pages_moved.fetch_add(1, Ordering::Relaxed);
    MigrateResult::Success {
        new_frame: dst_frame,
    }
}

/// Helper: ordered page allocation on a specific NUMA node.
/// Delegates to `BUDDY.alloc_on_node(order, flags, numa_node)`.
///
/// # Safety: called from migrate_page, CPL 0.
unsafe fn alloc_pages_on_node(node_id: u32, order: u32, flags: AllocFlags) -> Option<Frame> {
    use crate::memory::physical::BUDDY;
    // alloc_on_node(order: usize, flags: AllocFlags, numa_node: u8) → Result
    BUDDY
        .alloc_on_node(order as usize, flags, node_id as u8)
        .ok()
}

// ─────────────────────────────────────────────────────────────────────────────
// Batch migration
// ─────────────────────────────────────────────────────────────────────────────

/// Migrates up to `max_pages` frames to `target_node` from provided list.
/// Returns number of successfully migrated pages.
///
/// # Safety
/// Frames must be valid, unaliased during operation.
pub unsafe fn migrate_pages_batch<P: MigrationPageTableOps>(
    frames: &[(Frame, Option<u64>)], // (frame, optional virt_addr)
    target_node: u32,
    max_pages: usize,
    pt_ops: Option<&P>,
) -> usize {
    let mut count = 0;
    for &(frame, virt) in frames.iter().take(max_pages) {
        match migrate_page(frame, target_node, virt, pt_ops) {
            MigrateResult::Success { .. } => count += 1,
            MigrateResult::AlreadyLocal => count += 1, // already optimal
            _ => {}
        }
    }
    count
}

// ─────────────────────────────────────────────────────────────────────────────
// Initialization
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    // No structures to initialize for this module.
}
