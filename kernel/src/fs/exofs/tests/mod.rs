//! Tests unitaires, intégration et fuzz ExoFS — spec section 2.0.
//!
//! Structure :
//!   - `unit/`  — tests unitaires par sous-module
//!   - `integration/` — tests filière complète (write → commit → read → verify)
//!   - `fuzz/`  — seeds fuzz pour epoch_record, superblock, blob_id

// ── Tests unitaires ──────────────────────────────────────────────────────────
pub mod unit;

// ── Tests intégration ────────────────────────────────────────────────────────
pub mod integration;
