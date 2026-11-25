//! Internal ExoFS monotonic timestamping — delegates to `crate::fs::exofs::platform`.
//!
//! Zero inline assembly inside `fs/exofs/`: platform-specific TSC reading lives in
//! `crate::fs::exofs_hal::x86_platform` via `crate::arch::x86_64::cpu::tsc`.

/// Returns the current monotonic tick counter (CPU cycles since reset).
///
/// Used as a relative timestamp in:
/// - `relation/relation_gc.rs`  — relation age
/// - `relation/relation_batch.rs` — batch timestamps
/// - `audit/audit_rotation.rs`  — rotation trigger
/// - `audit/audit_writer.rs`    — audit entry timestamps
#[inline(always)]
pub fn exofs_ticks() -> u64 {
    crate::fs::exofs::platform::monotonic_ticks()
}
