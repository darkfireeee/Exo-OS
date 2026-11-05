// kernel/src/memory/numa.rs
//
// ─────────────────────────────────────────────────────────────────────────────
// NUMA — Re-export facade to memory/physical/numa/
// ─────────────────────────────────────────────────────────────────────────────
//
// This file contains NO implementation.
// The actual implementation is in memory/physical/numa/{node,distance,policy,migration}.
//
// RULE: Any reference to `memory::numa::X` is equivalent to
//       `memory::physical::numa::X` — transparent facade.

pub use crate::memory::physical::numa::*;
