//! Horodatage interne ExoFS — abstraction DAG-01 conforme.
//!
//! # Règle DAG-01
//! `fs/exofs/` ne peut dépendre QUE de `memory/`, `scheduler/`,
//! `security/capability/`. L'importation directe de `arch::time::read_ticks`
//! est une violation DAG-01.
//!
//! Ce module fournit `exofs_ticks() -> u64` via RDTSC inline asm — la lecture
//! du TSC est une instruction non-privilégiée disponible en Ring 0 sans passer
//! par le module `arch/`.
//!
//! # Sécurité
//! RDTSC peut ne pas être ordonné sur SMP (instruction non-sérialisante).
//! Pour un simple horodatage relatif  (durées GC, rotation logs, âge relations)
//! c'est suffisant — aucune ordering guarantee n'est requise.

/// Retourne le contador TSC courant (cycles CPU depuis le reset).
///
/// Utilisé comme horodatage relatif dans :
/// - `relation/relation_gc.rs`  — âge des relations
/// - `relation/relation_batch.rs` — timestamp des batches
/// - `audit/audit_rotation.rs`  — déclenchement de la rotation
/// - `audit/audit_writer.rs`    — timestamp des entrées d'audit
///
/// DAG-01 : pas d'import `crate::arch` — RDTSC via inline asm directement.
#[inline(always)]
pub fn exofs_ticks() -> u64 {
    // SAFETY: RDTSC est une instruction de lecture seule, non-privilégiée (pas
    //         besoin de Ring 0). Elle ne touche ni la mémoire ni le stack.
    //         `lfence` avant le RDTSC garantit que les instructions précédentes
    //         sont visibles sur le même cœur (sérialisation légère).
    // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
    unsafe {
        let lo: u32;
        let hi: u32;
        core::arch::asm!(
            "lfence",
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem, preserves_flags)
        );
        ((hi as u64) << 32) | (lo as u64)
    }
}
