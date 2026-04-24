// kernel/src/fs/exofs/epoch/epoch_pin.rs
//
// =============================================================================
// Épinglage d'Epochs — empêche le GC de collecter les objets d'un epoch
// Ring 0 · no_std · Exo-OS
// =============================================================================
//
// Un EpochPin est un guard acquis avant de lire des objets d'un epoch passé
// (snapshot, audit, export). Tant que le pin est tenu, le GC ne peut pas
// libérer les blocs de cet epoch.
//
// RÈGLE DEAD-01 : le GC vérifie la table de pins SANS tenir EPOCH_COMMIT_LOCK.
// RÈGLE LOCK-04 : EpochPinTable protégé par SpinLock léger.
// RÈGLE ARITH-02: saturating_sub pour décrémenter ref_count.
// RÈGLE OOM-02  : allocation statique (tableau fixe, pas de Vec).

use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::fs::exofs::core::{EpochId, ExofsError, ExofsResult};
use crate::fs::exofs::epoch::epoch_stats::EPOCH_STATS;
use crate::scheduler::sync::spinlock::SpinLock;

// =============================================================================
// Constantes
// =============================================================================

/// Nombre maximal de pins simultanés.
pub const MAX_EPOCH_PINS: usize = 64;

/// Sentinel indiquant un slot libre.
const SLOT_FREE: u64 = 0;

// =============================================================================
// PinEntry — entrée dans la table des pins
// =============================================================================

/// Raison de l'acquisition d'un pin.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PinReason {
    /// Pin acquis pour un snapshot.
    Snapshot = 0,
    /// Pin acquis pour une lecture audit.
    Audit = 1,
    /// Pin acquis pour un export.
    Export = 2,
    /// Pin acquis pour réplication.
    Replica = 3,
    /// Usage interne.
    Internal = 255,
}

impl fmt::Display for PinReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Snapshot => write!(f, "snapshot"),
            Self::Audit => write!(f, "audit"),
            Self::Export => write!(f, "export"),
            Self::Replica => write!(f, "replica"),
            Self::Internal => write!(f, "internal"),
        }
    }
}

/// Entrée dans la table des pins actifs.
#[derive(Copy, Clone, Debug)]
struct PinEntry {
    /// EpochId épinglé (0 = slot libre, cf SLOT_FREE).
    epoch_id: u64,
    /// Compteur de références sur ce pin.
    ref_count: u32,
    /// Token d'identification du pinner (snapshot_id, PID, etc.).
    owner: u32,
    /// Raison du pin.
    reason: PinReason,
    /// Timestamp d'acquisition (TSC).
    acquired_at: u64,
}

impl PinEntry {
    const fn empty() -> Self {
        Self {
            epoch_id: SLOT_FREE,
            ref_count: 0,
            owner: 0,
            reason: PinReason::Internal,
            acquired_at: 0,
        }
    }

    #[inline]
    fn is_free(self) -> bool {
        self.epoch_id == SLOT_FREE
    }
}

// =============================================================================
// PinTableInner — état interne de la table
// =============================================================================

struct PinTableInner {
    entries: [PinEntry; MAX_EPOCH_PINS],
    /// Nombre de slots occupés.
    count: usize,
    /// Total de pins acquis depuis le boot.
    total_acquired: u64,
    /// Total de pins relâchés depuis le boot.
    total_released: u64,
    /// Pic max de pins simultanés.
    peak_concurrent: u64,
}

impl PinTableInner {
    const fn new() -> Self {
        Self {
            entries: [PinEntry::empty(); MAX_EPOCH_PINS],
            count: 0,
            total_acquired: 0,
            total_released: 0,
            peak_concurrent: 0,
        }
    }

    /// Acquiert un pin pour l'epoch et le owner donnés.
    ///
    /// Retourne l'indice du slot alloué.
    fn pin(
        &mut self,
        epoch_id: EpochId,
        owner: u32,
        reason: PinReason,
        acquired_at: u64,
    ) -> ExofsResult<usize> {
        if epoch_id.0 == SLOT_FREE {
            return Err(ExofsError::InvalidPin);
        }
        // Cherche d'abord un slot déjà occupé avec le même (epoch_id, owner).
        for (i, entry) in self.entries.iter_mut().enumerate() {
            if !entry.is_free() && entry.epoch_id == epoch_id.0 && entry.owner == owner {
                entry.ref_count = entry.ref_count.saturating_add(1);
                return Ok(i);
            }
        }
        // Cherche un slot libre.
        for (i, entry) in self.entries.iter_mut().enumerate() {
            if entry.is_free() {
                *entry = PinEntry {
                    epoch_id: epoch_id.0,
                    ref_count: 1,
                    owner,
                    reason,
                    acquired_at,
                };
                self.count = self.count.saturating_add(1);
                self.total_acquired = self.total_acquired.saturating_add(1);
                if self.count as u64 > self.peak_concurrent {
                    self.peak_concurrent = self.count as u64;
                }
                return Ok(i);
            }
        }
        Err(ExofsError::TooManyPins)
    }

    /// Libère un pin par indice de slot.
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
            self.total_released = self.total_released.saturating_add(1);
        }
        Ok(())
    }

    /// Epoch minimum épinglé (pour le GC — ne pas collecter < cette valeur).
    fn oldest_pinned_epoch(&self) -> Option<EpochId> {
        // RÈGLE RECUR-01 : itération linéaire.
        let mut min_epoch: Option<u64> = None;
        for entry in &self.entries {
            if !entry.is_free() {
                min_epoch = Some(match min_epoch {
                    None => entry.epoch_id,
                    Some(prev) => prev.min(entry.epoch_id),
                });
            }
        }
        min_epoch.map(EpochId)
    }

    /// Vrai si l'epoch est actuellement épinglé.
    fn is_pinned(&self, epoch_id: EpochId) -> bool {
        self.entries
            .iter()
            .any(|e| !e.is_free() && e.epoch_id == epoch_id.0)
    }

    /// Nombre de pins actifs.
    #[inline]
    fn active_count(&self) -> usize {
        self.count
    }

    /// Retourne un snapshot des métriques.
    fn stats_snapshot(&self) -> PinTableStats {
        PinTableStats {
            active_pins: self.count as u64,
            peak_concurrent: self.peak_concurrent,
            total_acquired: self.total_acquired,
            total_released: self.total_released,
            oldest_pinned: self.oldest_pinned_epoch(),
        }
    }
}

// =============================================================================
// Singleton global
// =============================================================================

static EPOCH_PIN_TABLE: SpinLock<PinTableInner> = SpinLock::new(PinTableInner::new());

/// Compteur global de pins acquis (accessible sans lock pour monitoring).
static GLOBAL_PIN_COUNT: AtomicU64 = AtomicU64::new(0);

// =============================================================================
// EpochPin — guard RAII
// =============================================================================

/// Guard RAII : épingle un epoch pour la durée de vie de cet objet.
///
/// Quand l'EpochPin est droppé, l'epoch est automatiquement désépinglé.
pub struct EpochPin {
    epoch_id: EpochId,
    slot: usize,
    owner: u32,
    reason: PinReason,
}

impl EpochPin {
    /// Épingle l'epoch donné et retourne un guard RAII.
    ///
    /// # Erreurs
    /// - `ExofsError::TooManyPins` si la table est pleine.
    /// - `ExofsError::InvalidPin` si epoch_id == 0.
    pub fn acquire(epoch_id: EpochId, owner: u32) -> ExofsResult<Self> {
        Self::acquire_with_reason(epoch_id, owner, PinReason::Internal, 0)
    }

    /// Épingle avec une raison explicite et un timestamp.
    pub fn acquire_with_reason(
        epoch_id: EpochId,
        owner: u32,
        reason: PinReason,
        acquired_at: u64,
    ) -> ExofsResult<Self> {
        let mut table = EPOCH_PIN_TABLE.lock();
        let slot = table
            .pin(epoch_id, owner, reason, acquired_at)
            .map_err(|e| {
                EPOCH_STATS.inc_pins_failed();
                e
            })?;
        GLOBAL_PIN_COUNT.fetch_add(1, Ordering::Relaxed);
        EPOCH_STATS.inc_pins_acquired();
        let cur = table.active_count() as u64;
        EPOCH_STATS.update_pin_max(cur);
        Ok(Self {
            epoch_id,
            slot,
            owner,
            reason,
        })
    }

    /// Retourne l'EpochId épinglé.
    #[inline]
    pub fn epoch_id(&self) -> EpochId {
        self.epoch_id
    }

    /// Retourne l'owner du pin.
    #[inline]
    pub fn owner(&self) -> u32 {
        self.owner
    }

    /// Retourne la raison du pin.
    #[inline]
    pub fn reason(&self) -> PinReason {
        self.reason
    }
}

impl Drop for EpochPin {
    fn drop(&mut self) {
        let mut table = EPOCH_PIN_TABLE.lock();
        // L'erreur est ignorée : drop ne peut pas propager d'erreur.
        let _ = table.unpin(self.slot);
        GLOBAL_PIN_COUNT.fetch_sub(1, Ordering::Relaxed);
    }
}

impl fmt::Debug for EpochPin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "EpochPin{{ epoch={} owner={} reason={} slot={} }}",
            self.epoch_id.0, self.owner, self.reason, self.slot,
        )
    }
}

// =============================================================================
// PinSnapshot — vue instantanée d'un pin
// =============================================================================

/// Vue instantanée d'un pin actif (lecture seule, pour diagnostic).
#[derive(Copy, Clone, Debug)]
pub struct PinSnapshot {
    pub epoch_id: EpochId,
    pub owner: u32,
    pub ref_count: u32,
    pub reason: PinReason,
    pub acquired_at: u64,
}

/// Retourne les snapshots de tous les pins actifs.
///
/// RÈGLE RECUR-01 : itération linéaire.
pub fn list_active_pins() -> alloc::vec::Vec<PinSnapshot> {
    let table = EPOCH_PIN_TABLE.lock();
    let mut result = alloc::vec::Vec::new();
    for entry in &table.entries {
        if !entry.is_free() {
            let _ = result.try_reserve(1);
            result.push(PinSnapshot {
                epoch_id: EpochId(entry.epoch_id),
                owner: entry.owner,
                ref_count: entry.ref_count,
                reason: entry.reason,
                acquired_at: entry.acquired_at,
            });
        }
    }
    result
}

// =============================================================================
// API publique pour le GC et le scheduler
// =============================================================================

/// Retourne l'epoch le plus ancien actuellement épinglé, ou `None`.
///
/// Le GC utilise cette valeur pour ne pas collecter les objets créés
/// dans ou après cet epoch.
pub fn oldest_pinned_epoch() -> Option<EpochId> {
    EPOCH_PIN_TABLE.lock().oldest_pinned_epoch()
}

/// Vrai si l'epoch donné est actuellement épinglé.
pub fn is_epoch_pinned(epoch_id: EpochId) -> bool {
    EPOCH_PIN_TABLE.lock().is_pinned(epoch_id)
}

/// Nombre de pins actifs (lecture rapide depuis compteur atomique).
#[inline]
pub fn active_pin_count() -> u64 {
    GLOBAL_PIN_COUNT.load(Ordering::Relaxed)
}

/// Retourne les statistiques de la table de pins.
pub fn pin_table_stats() -> PinTableStats {
    EPOCH_PIN_TABLE.lock().stats_snapshot()
}

// =============================================================================
// PinTableStats — métriques de la table
// =============================================================================

/// Métriques non-atomiques de la table de pins.
#[derive(Copy, Clone, Debug)]
pub struct PinTableStats {
    /// Nombre de pins actifs en ce moment.
    pub active_pins: u64,
    /// Pic maximum de pins simultanés.
    pub peak_concurrent: u64,
    /// Total de pins acquis depuis le boot.
    pub total_acquired: u64,
    /// Total de pins relâchés depuis le boot.
    pub total_released: u64,
    /// Epoch le plus ancien épinglé (None = aucun pin actif).
    pub oldest_pinned: Option<EpochId>,
}

impl fmt::Display for PinTableStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PinTable{{ active={} peak={} acq={} rel={} oldest={:?} }}",
            self.active_pins,
            self.peak_concurrent,
            self.total_acquired,
            self.total_released,
            self.oldest_pinned.map(|e| e.0),
        )
    }
}

// =============================================================================
// Validation
// =============================================================================

/// Valide qu'un slot de pin est cohérent (pour le monitoring/assert).
pub fn validate_pin_table() -> bool {
    let table = EPOCH_PIN_TABLE.lock();
    let active = table.entries.iter().filter(|e| !e.is_free()).count();
    // Un slot actif ne doit jamais avoir epoch_id == 0.
    let no_zero = table
        .entries
        .iter()
        .filter(|e| !e.is_free())
        .all(|e| e.epoch_id != SLOT_FREE);
    active == table.count && no_zero
}
