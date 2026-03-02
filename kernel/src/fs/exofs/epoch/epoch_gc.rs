// kernel/src/fs/exofs/epoch/epoch_gc.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Interface GC → Epoch : calcul de la fenêtre de collection
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Ce module répond à la question clé du GC :
// "Jusqu'à quel epoch puis-je collecter en toute sécurité ?"
//
// RÈGLE DEAD-01 : Ce module NE JAMAIS acquiert EPOCH_COMMIT_LOCK.
//                 Il lit uniquement epoch atomiques et la table des pins.
// RÈGLE GC-04   : On ne collecte jamais un epoch épinglé par un snapshot.

use crate::fs::exofs::core::{ExofsResult, EpochId};
use crate::fs::exofs::epoch::epoch_pin::oldest_pinned_epoch;
use crate::fs::exofs::storage::superblock::ExoSuperblockInMemory;

// ─────────────────────────────────────────────────────────────────────────────
// Fenêtre de collection
// ─────────────────────────────────────────────────────────────────────────────

/// Fenêtre d'epochs collectables calculée par le planificateur GC.
#[derive(Copy, Clone, Debug)]
pub struct GcEpochWindow {
    /// Premier epoch collectable (inclus).
    pub from_epoch: EpochId,
    /// Dernier epoch collectable (exclus).
    pub until_epoch: EpochId,
    /// Nombre d'epochs dans la fenêtre.
    pub count: u64,
}

/// Calcule la fenêtre d'epochs sûrs à collecter.
///
/// La fenêtre est bornée par :
/// - Le bas : epoch = 0 (ou le premier epoch non-vide).
/// - Le haut : min(epoch_courant - 1, oldest_pinned_epoch - 1).
///
/// RÈGLE DEAD-01 : lecture atomique du superblock seulement, PAS de lock epoch.
pub fn compute_gc_window(superblock: &ExoSuperblockInMemory) -> GcEpochWindow {
    let current = superblock.current_epoch();
    let current_val = current.0;

    // Epoch courant n'est jamais collectable.
    let upper_exclusive = if current_val == 0 { 0 } else { current_val };

    // Contrainte supplémentaire : ne pas dépasser un epoch épinglé par snapshot.
    let upper = match oldest_pinned_epoch() {
        Some(pinned) if pinned.0 < upper_exclusive => pinned.0,
        _ => upper_exclusive,
    };

    if upper == 0 {
        // Rien à collecter.
        return GcEpochWindow {
            from_epoch:  EpochId(0),
            until_epoch: EpochId(0),
            count:       0,
        };
    }

    GcEpochWindow {
        from_epoch:  EpochId(0),
        until_epoch: EpochId(upper),
        count:       upper,
    }
}

/// Vrai si l'epoch donné est dans la fenêtre de collection.
#[inline]
pub fn epoch_is_collectable(epoch: EpochId, window: &GcEpochWindow) -> bool {
    window.count > 0
        && epoch.0 >= window.from_epoch.0
        && epoch.0 < window.until_epoch.0
}

/// Calcule le lag entre l'epoch courant et l'epoch le plus ancien en attente.
///
/// Un lag élevé indique que le GC est en retard.
pub fn gc_epoch_lag(superblock: &ExoSuperblockInMemory, oldest_uncollected: EpochId) -> u64 {
    let current = superblock.current_epoch().0;
    current.saturating_sub(oldest_uncollected.0)
}
