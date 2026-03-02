// kernel/src/fs/exofs/epoch/epoch_pin.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Épinglage d'Epochs — empêche le GC de collecter les objets d'un epoch
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Un EpochPin est un guard acquis avant de lire des objets d'un epoch passé
// (snapshot, audit, export). Tant que le pin est tenu, le GC ne peut pas
// libérer les blocs de cet epoch.
//
// RÈGLE DEAD-01 : le GC vérifie la table de pins SANS tenir EPOCH_COMMIT_LOCK.
// RÈGLE LOCK-04 : EpochPinTable protégé par SpinLock léger.

use core::sync::atomic::{AtomicU64, Ordering};
use alloc::vec::Vec;

use crate::fs::exofs::core::{ExofsError, ExofsResult, EpochId};
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximal de pins simultanés.
const MAX_EPOCH_PINS: usize = 64;

// ─────────────────────────────────────────────────────────────────────────────
// EpochPinTable — table globale des pins actifs
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug)]
struct PinEntry {
    /// EpochId épinglé (0 = slot libre).
    epoch_id: u64,
    /// Compteur de références sur ce pin.
    ref_count: u32,
    /// Token d'identification du pinner (PID ou handle).
    owner: u32,
}

impl PinEntry {
    const fn empty() -> Self {
        Self { epoch_id: 0, ref_count: 0, owner: 0 }
    }
    fn is_free(self) -> bool {
        self.epoch_id == 0
    }
}

struct PinTableInner {
    entries: [PinEntry; MAX_EPOCH_PINS],
    count:   usize,
}

impl PinTableInner {
    const fn new() -> Self {
        Self {
            entries: [PinEntry::empty(); MAX_EPOCH_PINS],
            count:   0,
        }
    }

    fn pin(&mut self, epoch_id: EpochId, owner: u32) -> ExofsResult<usize> {
        // Cherche un slot libre.
        for (i, entry) in self.entries.iter_mut().enumerate() {
            if entry.is_free() {
                *entry = PinEntry { epoch_id: epoch_id.0, ref_count: 1, owner };
                self.count += 1;
                return Ok(i);
            }
        }
        Err(ExofsError::TooManyPins)
    }

    fn unpin(&mut self, slot: usize) -> ExofsResult<()> {
        if slot >= MAX_EPOCH_PINS {
            return Err(ExofsError::InvalidPin);
        }
        let entry = &mut self.entries[slot];
        if entry.is_free() {
            return Err(ExofsError::InvalidPin);
        }
        entry.ref_count = entry.ref_count.saturating_sub(1);
        if entry.ref_count == 0 {
            *entry = PinEntry::empty();
            self.count = self.count.saturating_sub(1);
        }
        Ok(())
    }

    /// Epoch minimum épinglé (pour le GC — ne pas collecter < cette valeur).
    fn oldest_pinned_epoch(&self) -> Option<EpochId> {
        self.entries
            .iter()
            .filter(|e| !e.is_free())
            .map(|e| EpochId(e.epoch_id))
            .min_by_key(|e| e.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Singleton global
// ─────────────────────────────────────────────────────────────────────────────

static EPOCH_PIN_TABLE: SpinLock<PinTableInner> =
    SpinLock::new(PinTableInner::new());

// ─────────────────────────────────────────────────────────────────────────────
// EpochPin — guard RAII
// ─────────────────────────────────────────────────────────────────────────────

/// Guard RAII : épingle un epoch pour la durée de vie de cet objet.
///
/// Quand l'EpochPin est droppé, l'epoch est automatiquement désépinglé.
pub struct EpochPin {
    epoch_id: EpochId,
    slot:     usize,
}

impl EpochPin {
    /// Épingle l'epoch donné. Retourne un guard RAII.
    pub fn acquire(epoch_id: EpochId, owner: u32) -> ExofsResult<Self> {
        let mut table = EPOCH_PIN_TABLE.lock();
        let slot = table.pin(epoch_id, owner)?;
        Ok(Self { epoch_id, slot })
    }

    /// Retourne l'EpochId épinglé.
    #[inline]
    pub fn epoch_id(&self) -> EpochId {
        self.epoch_id
    }
}

impl Drop for EpochPin {
    fn drop(&mut self) {
        let mut table = EPOCH_PIN_TABLE.lock();
        // L'erreur ici est ignorée : le drop ne peut pas propager d'erreur.
        let _ = table.unpin(self.slot);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// API publique pour le GC
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne l'epoch le plus ancien actuellement épinglé, ou `None`.
///
/// Le GC utilise cette valeur pour ne pas collecter les objets créés après
/// l'epoch le plus ancient épinglé.
pub fn oldest_pinned_epoch() -> Option<EpochId> {
    let table = EPOCH_PIN_TABLE.lock();
    table.oldest_pinned_epoch()
}

/// Vrai si l'epoch donné est actuellement épinglé par au moins un pinner.
pub fn is_epoch_pinned(epoch_id: EpochId) -> bool {
    let table = EPOCH_PIN_TABLE.lock();
    table.entries.iter().any(|e| !e.is_free() && e.epoch_id == epoch_id.0)
}
