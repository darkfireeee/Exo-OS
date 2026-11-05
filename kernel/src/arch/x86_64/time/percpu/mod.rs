// kernel/src/arch/x86_64/time/percpu/mod.rs
//
// ════════════════════════════════════════════════════════════════════════════
// Per-CPU data related to timekeeping
// ════════════════════════════════════════════════════════════════════════════
//
// This module provides:
//   sync.rs   — inter-CPU TSC offset measurement at SMP boot (RULE TSC-SYNC-01)
//   tsc_offset — re-exported from ktime so ktime_get_ns() can call it
//               via path `super::percpu::tsc_offset(coreid)`
//
// ## Dependency path
//   ktime::ktime_get_ns()
//     └─ super::percpu::tsc_offset(coreid)   ← this module
//          └─ super::ktime::tsc_offset(cpu)  ← ktime.rs (real storage)
// ════════════════════════════════════════════════════════════════════════════

pub mod sync;

// Re-export of tsc_offset from ktime.
// ktime_get_ns() calls `super::percpu::tsc_offset(coreid as usize)`.
// Real storage is in ktime::TSC_OFFSETS (atomic array).
pub use super::ktime::tsc_offset;
pub use super::ktime::tsc_offset_valid;

// Public re-exports for SMP boot usage.
pub use sync::{ap_sync_tsc_response, init_bsp_percpu, measure_tsc_offset_for_ap, tsc_synced};
