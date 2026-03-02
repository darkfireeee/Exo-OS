//! epoch_id — compteur d'EpochId courant pour le module epoch (no_std).

use core::sync::atomic::{AtomicU64, Ordering};
use crate::fs::exofs::core::EpochId;

/// EpochId courant du système de fichiers (incrémenté à chaque commit).
static CURRENT_EPOCH: AtomicU64 = AtomicU64::new(1);

/// Lit l'EpochId courant sans l'incrémenter.
#[inline]
pub fn current_epoch_id() -> EpochId {
    EpochId(CURRENT_EPOCH.load(Ordering::Acquire))
}

/// Alloue l'EpochId du prochain commit (ancienne valeur + 1).
/// Retourne l'EpochId alloué.
pub fn allocate_next_epoch_id() -> EpochId {
    let prev = CURRENT_EPOCH.fetch_add(1, Ordering::AcqRel);
    EpochId(prev + 1)
}

/// Force la valeur de l'EpochId courant (utilisé lors du recovery).
/// `new_id` DOIT être ≥ valeur courante, sinon ignoré.
pub fn set_epoch_id_from_recovery(new_id: EpochId) {
    let _ = CURRENT_EPOCH.fetch_update(Ordering::AcqRel, Ordering::Acquire, |old| {
        if new_id.0 > old { Some(new_id.0) } else { None }
    });
}

/// Vérifie que `candidate` est un EpochId futur valide (> courant).
#[inline]
pub fn is_future_epoch(candidate: EpochId) -> bool {
    candidate.0 > CURRENT_EPOCH.load(Ordering::Relaxed)
}

/// Vérifie que `candidate` est dans la fenêtre [current - window, current].
#[inline]
pub fn epoch_within_grace(candidate: EpochId, window: u64) -> bool {
    let cur = CURRENT_EPOCH.load(Ordering::Relaxed);
    candidate.0 <= cur && cur.saturating_sub(candidate.0) <= window
}
