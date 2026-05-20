//! SSR — Shared State Region ExoPhoenix (A <-> B)

use core::sync::atomic::{AtomicU32, AtomicU64};

use crate::memory::{phys_to_virt, PhysAddr};

pub use exo_phoenix_ssr::{
    HANDOFF_B_ACTIVE, HANDOFF_FREEZE_ACK_ALL, HANDOFF_FREEZE_REQ, HANDOFF_NORMAL,
    SSR_BASE_PHYS as SSR_BASE, SSR_CMD_B2A_OFFSET as SSR_CMD_B2A,
    SSR_FREEZE_ACK_OFFSET as SSR_FREEZE_ACK, SSR_HANDOFF_FLAG_OFFSET as SSR_HANDOFF_FLAG,
    SSR_LAYOUT_MAJOR, SSR_LAYOUT_MINOR, SSR_LIVENESS_NONCE_OFFSET as SSR_LIVENESS_NONCE,
    SSR_LOG_AUDIT_OFFSET as SSR_LOG_AUDIT, SSR_MAGIC_OFFSET as SSR_MAGIC, SSR_MAGIC_VERSION,
    SSR_MAX_CORES_LAYOUT as MAX_CORES, SSR_METRICS_OFFSET as SSR_METRICS_PUSH,
    SSR_PMC_OFFSET as SSR_PMC_SNAPSHOT, SSR_SEQLOCK_OFFSET as SSR_SEQLOCK, SSR_SIZE,
};

pub const FREEZE_ACK_DONE: u32 = 0xACED_0001;
pub const TLB_ACK_DONE: u32 = 0xACED_0002;

/// Process records retained for recovery metadata during a Phoenix switch.
pub const SSR_MAX_PROCESSES: usize = 24;
/// IPC endpoints retained for recovery metadata during a Phoenix switch.
pub const SSR_MAX_ENDPOINTS: usize = 48;
pub const SSR_PROCESS_RECORD_SIZE: usize = 96;
pub const SSR_ENDPOINT_RECORD_SIZE: usize = 24;
pub const SSR_RECOVERY_STATE_SIZE: usize = 64
    + 44
    + 4
    + SSR_MAX_PROCESSES * SSR_PROCESS_RECORD_SIZE
    + 4
    + SSR_MAX_ENDPOINTS * SSR_ENDPOINT_RECORD_SIZE
    + 16;

const _: () = assert!(
    SSR_SIZE <= 16 * 4096,
    "SSR physical layout must fit in the reserved 64 KiB region"
);
const _: () = assert!(
    SSR_RECOVERY_STATE_SIZE <= 4096,
    "SSR recovery metadata budget must fit in one 4 KiB page"
);
const _: () = assert!(
    SSR_MAX_PROCESSES >= 12,
    "SSR must preserve all Ring1 services before optional Ring3 records"
);
const _: () = assert!(
    MAX_CORES == crate::arch::constants::SSR_MAX_CORES_LAYOUT,
    "SSR core layout must match arch constants"
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SsrVersionError {
    IncompatibleMagicVersion(u64),
}

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

#[inline(always)]
pub fn read_magic_version() -> u64 {
    // SAFETY: SSR_MAGIC est un offset u64 valide dans la SSR v7.
    unsafe { ssr_atomic(SSR_MAGIC).load(core::sync::atomic::Ordering::Acquire) }
}

#[inline(always)]
pub fn write_magic_version() {
    // SAFETY: SSR_MAGIC est un offset u64 valide dans la SSR v7.
    unsafe {
        ssr_atomic(SSR_MAGIC).store(
            exo_phoenix_ssr::SSR_MAGIC_VERSION,
            core::sync::atomic::Ordering::Release,
        );
    }
}

pub fn validate_layout_v7() -> Result<(), SsrVersionError> {
    let value = read_magic_version();
    if exo_phoenix_ssr::is_compatible_magic_version(value) {
        Ok(())
    } else {
        Err(SsrVersionError::IncompatibleMagicVersion(value))
    }
}

/// Initialise la SSR détenue par Kernel B, puis valide le contrat v7 compilé.
///
/// Cette fonction ne touche pas aux zones audit/métriques: elle initialise le
/// header et les atomics de synchronisation nécessaires au handoff.
pub fn initialize_layout_v7() -> Result<(), SsrVersionError> {
    let observed = read_magic_version();
    if exo_phoenix_ssr::magic_from_magic_version(observed) == exo_phoenix_ssr::SSR_MAGIC
        && !exo_phoenix_ssr::is_compatible_magic_version(observed)
    {
        return Err(SsrVersionError::IncompatibleMagicVersion(observed));
    }

    write_magic_version();

    // SAFETY: offsets v7 bornés par les assertions de `exo-phoenix-ssr`.
    unsafe {
        ssr_atomic(SSR_HANDOFF_FLAG).store(HANDOFF_NORMAL, core::sync::atomic::Ordering::Release);
        ssr_atomic(SSR_LIVENESS_NONCE).store(0, core::sync::atomic::Ordering::Release);
        ssr_atomic(SSR_SEQLOCK).store(0, core::sync::atomic::Ordering::Release);

        for slot in 0..MAX_CORES {
            ssr_atomic_u32(freeze_ack_offset(slot)).store(0, core::sync::atomic::Ordering::Release);
        }
    }

    validate_layout_v7()
}
