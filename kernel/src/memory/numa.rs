// kernel/src/memory/numa.rs
//
// ─────────────────────────────────────────────────────────────────────────────
// NUMA — Façade de re-export vers memory/physical/numa/
// ─────────────────────────────────────────────────────────────────────────────
//
// Ce fichier ne contient AUCUNE implémentation.
// L'implémentation réelle est dans memory/physical/numa/{node,distance,policy,migration}.
//
// RÈGLE : Toute référence à `memory::numa::X` est équivalente à
//         `memory::physical::numa::X` — façade transparente.

pub use crate::memory::physical::numa::*;
