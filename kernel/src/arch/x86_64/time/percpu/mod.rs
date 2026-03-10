// kernel/src/arch/x86_64/time/percpu/mod.rs
//
// ════════════════════════════════════════════════════════════════════════════
// Données per-CPU liées au timekeeping
// ════════════════════════════════════════════════════════════════════════════
//
// Ce module fournit :
//   sync.rs   — mesure du décalage TSC inter-CPU au boot SMP (RÈGLE TSC-SYNC-01)
//   tsc_offset — re-exporté depuis ktime pour que ktime_get_ns() puisse l'appeler
//               via le chemin `super::percpu::tsc_offset(coreid)`
//
// ## Chemin de dépendance
//   ktime::ktime_get_ns()
//     └─ super::percpu::tsc_offset(coreid)   ← ce module
//          └─ super::ktime::tsc_offset(cpu)  ← ktime.rs (stockage réel)
// ════════════════════════════════════════════════════════════════════════════

pub mod sync;

// Re-export de tsc_offset depuis ktime.
// ktime_get_ns() appelle `super::percpu::tsc_offset(coreid as usize)`.
// Le stockage réel est dans ktime::TSC_OFFSETS (atomic array).
pub use super::ktime::tsc_offset;
pub use super::ktime::tsc_offset_valid;

// Re-exports publics pour les usages SMP boot.
pub use sync::{
    measure_tsc_offset_for_ap,
    ap_sync_tsc_response,
    init_bsp_percpu,
    tsc_synced,
};
