//! SSR — Shared State Region ExoPhoenix (A <-> B)

use core::sync::atomic::AtomicU64;

pub const SSR_BASE: u64 = 0x100_0000;
pub const SSR_SIZE: usize = 0x10000;
pub const MAX_CORES: usize = 64;

// Offsets SSR
pub const SSR_HANDOFF_FLAG: usize = 0x0000;
pub const SSR_LIVENESS_NONCE: usize = 0x0008;
pub const SSR_SEQLOCK: usize = 0x0010;
pub const SSR_CMD_B2A: usize = 0x0040;
pub const SSR_FREEZE_ACK: usize = 0x0080;
pub const SSR_PMC_SNAPSHOT: usize = 0x1080;
pub const SSR_LOG_AUDIT: usize = 0x8000;
pub const SSR_METRICS_PUSH: usize = 0xC000;

pub const FREEZE_ACK_DONE: u64 = 0xACED_0001;
pub const TLB_ACK_DONE: u64 = 0xACED_0002;

#[inline(always)]
pub fn freeze_ack_offset(slot_index: usize) -> usize {
    SSR_FREEZE_ACK + slot_index * 64
}

#[inline(always)]
pub fn pmc_snapshot_offset(slot_index: usize) -> usize {
    SSR_PMC_SNAPSHOT + slot_index * 64
}

/// Accès atomique à une case SSR.
///
/// # Safety
/// L'appelant doit fournir un offset valide et s'assurer que la SSR est mappée.
pub unsafe fn ssr_atomic(offset: usize) -> &'static AtomicU64 {
    &*((SSR_BASE as usize + offset) as *const AtomicU64)
}
