//! SSR — Shared State Region ExoPhoenix (A <-> B)

use core::sync::atomic::{AtomicU32, AtomicU64};

use crate::memory::{phys_to_virt, PhysAddr};

pub use exo_phoenix_ssr::{
    SSR_BASE_PHYS as SSR_BASE, SSR_CMD_B2A_OFFSET as SSR_CMD_B2A,
    SSR_FREEZE_ACK_OFFSET as SSR_FREEZE_ACK, SSR_HANDOFF_FLAG_OFFSET as SSR_HANDOFF_FLAG,
    SSR_LIVENESS_NONCE_OFFSET as SSR_LIVENESS_NONCE, SSR_LOG_AUDIT_OFFSET as SSR_LOG_AUDIT,
    SSR_MAX_CORES_LAYOUT as MAX_CORES, SSR_METRICS_OFFSET as SSR_METRICS_PUSH,
    SSR_PMC_OFFSET as SSR_PMC_SNAPSHOT, SSR_SIZE,
    SSR_SEQLOCK_OFFSET as SSR_SEQLOCK,
};

pub const FREEZE_ACK_DONE: u32 = 0xACED_0001;
pub const TLB_ACK_DONE: u32 = 0xACED_0002;

#[inline(always)]
pub fn freeze_ack_offset(slot_index: usize) -> usize {
    exo_phoenix_ssr::freeze_ack_offset(slot_index as u32)
}

#[inline(always)]
pub fn pmc_snapshot_offset(slot_index: usize) -> usize {
    exo_phoenix_ssr::pmc_snapshot_offset(slot_index as u32)
}

/// Accès atomique 64-bit à une case SSR (flags/nonce/compteurs 64-bit).
///
/// # Safety
/// L'appelant doit fournir un offset valide et s'assurer que la SSR est mappée.
pub unsafe fn ssr_atomic(offset: usize) -> &'static AtomicU64 {
    debug_assert!(offset + core::mem::size_of::<AtomicU64>() <= SSR_SIZE);
    let base = phys_to_virt(PhysAddr::new(SSR_BASE)).as_u64() as usize;
    &*((base + offset) as *const AtomicU64)
}

/// Accès atomique 32-bit à une case SSR (freeze ACK layout partagé: u32 × N).
///
/// # Safety
/// L'appelant doit fournir un offset valide et s'assurer que la SSR est mappée.
pub unsafe fn ssr_atomic_u32(offset: usize) -> &'static AtomicU32 {
    debug_assert!(offset + core::mem::size_of::<AtomicU32>() <= SSR_SIZE);
    let base = phys_to_virt(PhysAddr::new(SSR_BASE)).as_u64() as usize;
    &*((base + offset) as *const AtomicU32)
}
