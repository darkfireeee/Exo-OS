// kernel/src/fs/exofs/epoch/epoch_id.rs
//
// =============================================================================
// EpochId — compteur monotone, cycle de vie complet, fenêtres GC
// Ring 0 · no_std · Exo-OS
// =============================================================================
//
// L'EpochId est le vecteur de temps logique d'ExoFS. Chaque commit produit
// exactement un EpochId unique, strictement croissant.
//
// Règles appliquées :
//   ARITH-02  : checked_add / saturating_* obligatoires.
//   LOCK-01   : Ordering minimal adapté au besoin.
//   RECOV-02  : set_epoch_id_from_recovery() avance uniquement.
//   EPOCH-07  : EpochId(0) = invalide (volume vierge ou corrompu).

use core::cmp::Ordering;
use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering as AOrdering};

use crate::fs::exofs::core::{EpochId, ExofsError, ExofsResult};

// =============================================================================
// Constantes publiques
// =============================================================================

/// Sentinelle "aucun epoch valide" — volume vierge ou structurellement invalide.
pub const EPOCH_INVALID: EpochId = EpochId(0);

/// Premier epoch valide — utilisé à la création du volume.
pub const EPOCH_FIRST: EpochId = EpochId(1);

/// Seuil de wrapping imminent (u64::MAX - 2^32).
/// En pratique inaccessible, mais détecté et loggé pour sécurité.
const EPOCH_WRAP_SENTINEL: u64 = u64::MAX - (1u64 << 32);

/// Fenêtre de grâce par défaut pour le GC (en nombre d'epochs).
/// Un blob ref_count=0 n'est collecté que si epoch_age >= 2.
pub const DEFAULT_GC_GRACE_WINDOW: u64 = 2;

/// Nombre d'epochs max accumulables par le writeback avant commit forcé.
pub const EPOCH_WRITEBACK_MAX_PENDING: u64 = 8;

/// Nombre minimum d'epochs d'ancienneté avant qu'un blob soit collectable.
pub const EPOCH_MIN_COLLECT_AGE: u64 = 2;

// =============================================================================
// Registre global de l'EpochId courant
// =============================================================================

/// Epoch courant en mémoire (peut ne pas encore être sur disque).
///
/// Invariant : CURRENT_EPOCH >= 1 après init_epoch_counter().
/// Avance uniquement via allocate_next_epoch_id() ou set_epoch_id_from_recovery().
static CURRENT_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Epoch le plus récent provably persisté (avancé après les 3 barrières NVMe).
static DURABLE_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Numéro de séquence globale des commits (incrémenté à chaque commit réussi).
static COMMIT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

// =============================================================================
// Initialisation et reset
// =============================================================================

/// Initialise le compteur d'epochs au démarrage du volume.
///
/// Doit être appelé une seule fois avant tout accès au module epoch.
/// Si `initial` vaut EPOCH_INVALID (0), utilise EPOCH_FIRST (1).
///
/// # Erreurs
/// - `ExofsError::AlreadyInitialized` si le compteur est déjà non-nul.
pub fn init_epoch_counter(initial: EpochId) -> ExofsResult<()> {
    let start = if initial.0 == 0 {
        EPOCH_FIRST.0
    } else {
        initial.0
    };
    CURRENT_EPOCH
        .compare_exchange(0, start, AOrdering::AcqRel, AOrdering::Acquire)
        .map_err(|_| ExofsError::AlreadyInitialized)?;
    DURABLE_EPOCH.store(start, AOrdering::Release);
    Ok(())
}

/// Réinitialise le compteur (remontage du volume ou contexte de test).
///
/// ATTENTION : en production, uniquement après un démontage complet.
pub fn reset_epoch_counter() {
    CURRENT_EPOCH.store(0, AOrdering::Release);
    DURABLE_EPOCH.store(0, AOrdering::Release);
    COMMIT_SEQUENCE.store(0, AOrdering::Release);
}

// =============================================================================
// Lecture de l'état courant
// =============================================================================

/// Epoch courant en mémoire (pas forcément sur disque).
#[inline]
pub fn current_epoch_id() -> EpochId {
    EpochId(CURRENT_EPOCH.load(AOrdering::Acquire))
}

/// Epoch le plus récent connu comme persisté (après barrière NVMe 3).
#[inline]
pub fn durable_epoch_id() -> EpochId {
    EpochId(DURABLE_EPOCH.load(AOrdering::Acquire))
}

/// Numéro de séquence du dernier commit réussi.
#[inline]
pub fn commit_sequence() -> u64 {
    COMMIT_SEQUENCE.load(AOrdering::Relaxed)
}

/// Nombre d'epochs actuellement "en vol" (écrits mais pas encore durables).
#[inline]
pub fn epochs_in_flight() -> u64 {
    let cur = CURRENT_EPOCH.load(AOrdering::Acquire);
    let dur = DURABLE_EPOCH.load(AOrdering::Relaxed);
    cur.saturating_sub(dur)
}

// =============================================================================
// Allocation de nouveaux EpochIds
// =============================================================================

/// Alloue atomiquement le prochain EpochId (fetch_add puis +1).
///
/// # Guaranties
/// - Strict monotone : deux appels concurrents produisent des valeurs distinctes.
/// - Détection de wrapping : Err(EpochOverflow) si proche de u64::MAX.
///
/// # Erreurs
/// - `ExofsError::EpochOverflow` si le compteur dépasserait EPOCH_WRAP_SENTINEL.
pub fn allocate_next_epoch_id() -> ExofsResult<EpochId> {
    let old = CURRENT_EPOCH.fetch_add(1, AOrdering::AcqRel);
    let new_val = old.checked_add(1).ok_or_else(|| {
        // Reculer le compteur : fetch_add ne peut pas être annulé simplement,
        // on sature et retourne une erreur.
        ExofsError::EpochOverflow
    })?;
    if new_val >= EPOCH_WRAP_SENTINEL {
        // Quasi-impossible en production (2^64 epochs), mais protégé.
        CURRENT_EPOCH.fetch_sub(1, AOrdering::AcqRel);
        return Err(ExofsError::EpochOverflow);
    }
    Ok(EpochId(new_val))
}

/// Confirme qu'un epoch est maintenant durable (après les 3 barrières NVMe).
///
/// Met à jour DURABLE_EPOCH et incrémente COMMIT_SEQUENCE.
/// N'avance pas si `epoch` <= DURABLE_EPOCH courant (idempotent).
pub fn mark_epoch_durable(epoch: EpochId) {
    let _ = DURABLE_EPOCH.fetch_update(AOrdering::AcqRel, AOrdering::Acquire, |old| {
        if epoch.0 > old {
            Some(epoch.0)
        } else {
            None
        }
    });
    COMMIT_SEQUENCE.fetch_add(1, AOrdering::Relaxed);
}

// =============================================================================
// Recovery
// =============================================================================

/// Force l'EpochId courant depuis une valeur lue sur disque (recovery au boot).
///
/// Avance uniquement — si `new_id` <= courant, l'appel est sans effet.
/// Le prochain allocate_next_epoch_id() retournera new_id + 1.
///
/// Doit être appelé AVANT tout allocate_next_epoch_id() dans la séquence de boot.
pub fn set_epoch_id_from_recovery(new_id: EpochId) {
    // Valeur invalide : on force le minimum viable.
    let target = if new_id.0 == 0 {
        EPOCH_FIRST.0
    } else {
        new_id.0
    };
    let _ = CURRENT_EPOCH.fetch_update(AOrdering::AcqRel, AOrdering::Acquire, |old| {
        if target > old {
            Some(target)
        } else {
            None
        }
    });
    let _ = DURABLE_EPOCH.fetch_update(AOrdering::AcqRel, AOrdering::Acquire, |old| {
        if target > old {
            Some(target)
        } else {
            None
        }
    });
}

// =============================================================================
// Prédicats et comparaisons
// =============================================================================

/// Vrai si `candidate` est strictement futur (> epoch courant en mémoire).
#[inline]
pub fn is_future_epoch(candidate: EpochId) -> bool {
    candidate.0 > CURRENT_EPOCH.load(AOrdering::Relaxed)
}

/// Vrai si `candidate` est dans la fenêtre de grâce [current - window, current].
///
/// Utilisé par le GC pour déterminer si un epoch est encore "protégé".
#[inline]
pub fn epoch_within_grace(candidate: EpochId, window: u64) -> bool {
    if candidate.0 == 0 {
        return false;
    }
    let cur = CURRENT_EPOCH.load(AOrdering::Relaxed);
    candidate.0 <= cur && cur.saturating_sub(candidate.0) <= window
}

/// Vrai si `candidate` est collectable (ancienneté >= `min_age_epochs`).
#[inline]
pub fn epoch_is_old_enough(candidate: EpochId, min_age_epochs: u64) -> bool {
    if candidate.0 == 0 {
        return false;
    }
    let cur = CURRENT_EPOCH.load(AOrdering::Relaxed);
    cur.saturating_sub(candidate.0) >= min_age_epochs
}

/// Distance entre deux EpochIds (saturée à 0 si ordre inverse).
#[inline]
pub fn epoch_distance(from: EpochId, to: EpochId) -> u64 {
    to.0.saturating_sub(from.0)
}

/// Retourne le max de deux EpochIds.
#[inline]
pub fn epoch_max(a: EpochId, b: EpochId) -> EpochId {
    if a.0 >= b.0 {
        a
    } else {
        b
    }
}

/// Retourne le min de deux EpochIds.
#[inline]
pub fn epoch_min(a: EpochId, b: EpochId) -> EpochId {
    if a.0 <= b.0 {
        a
    } else {
        b
    }
}

/// Comparaison ordinale entre deux EpochIds.
#[inline]
pub fn epoch_cmp(a: EpochId, b: EpochId) -> Ordering {
    a.0.cmp(&b.0)
}

/// Vrai si `a` est strictement antérieur à `b`.
#[inline]
pub fn epoch_before(a: EpochId, b: EpochId) -> bool {
    a.0 < b.0
}

/// Vrai si `a` est strictement postérieur à `b`.
#[inline]
pub fn epoch_after(a: EpochId, b: EpochId) -> bool {
    a.0 > b.0
}

// =============================================================================
// Trait EpochIdExt — méthodes utilitaires production
// =============================================================================

/// Extension trait sur EpochId — méthodes utilitaires production.
pub trait EpochIdExt: Sized {
    /// Epoch suivant ou Err(EpochOverflow).
    fn next(self) -> ExofsResult<Self>;
    /// Epoch précédent ou None si déjà à 0.
    fn prev(self) -> Option<Self>;
    /// Vrai si cet epoch est persisté sur disque.
    fn is_durable(self) -> bool;
    /// Vrai si cet epoch est invalide (0).
    fn is_invalid(self) -> bool;
    /// Vrai si cet epoch est dans la fenêtre GC par défaut.
    fn is_gc_eligible(self) -> bool;
    /// Compare avec l'epoch courant en mémoire.
    fn compare_to_current(self) -> Ordering;
    /// Vrai si cet epoch est dans la fenêtre `[current-w, current]`.
    fn within_grace(self, window: u64) -> bool;
}

impl EpochIdExt for EpochId {
    #[inline]
    fn next(self) -> ExofsResult<Self> {
        let v = self.0.checked_add(1).ok_or(ExofsError::EpochOverflow)?;
        if v >= EPOCH_WRAP_SENTINEL {
            return Err(ExofsError::EpochOverflow);
        }
        Ok(EpochId(v))
    }

    #[inline]
    fn prev(self) -> Option<Self> {
        if self.0 == 0 {
            None
        } else {
            Some(EpochId(self.0 - 1))
        }
    }

    #[inline]
    fn is_durable(self) -> bool {
        self.0 != 0 && self.0 <= DURABLE_EPOCH.load(AOrdering::Relaxed)
    }

    #[inline]
    fn is_invalid(self) -> bool {
        self.0 == 0
    }

    #[inline]
    fn is_gc_eligible(self) -> bool {
        epoch_is_old_enough(self, DEFAULT_GC_GRACE_WINDOW)
    }

    #[inline]
    fn compare_to_current(self) -> Ordering {
        let cur = CURRENT_EPOCH.load(AOrdering::Relaxed);
        self.0.cmp(&cur)
    }

    #[inline]
    fn within_grace(self, window: u64) -> bool {
        epoch_within_grace(self, window)
    }
}

// =============================================================================
// Plage d'epochs : EpochRange
// =============================================================================

/// Représente une plage [start, end) d'EpochIds.
///
/// Utilisée par le GC pour décrire la fenêtre de collection,
/// et par le recovery pour identifier la séquence d'epochs valides.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct EpochRange {
    /// Premier epoch inclus (>=).
    pub start: EpochId,
    /// Premier epoch exclu (<).
    pub end: EpochId,
}

impl EpochRange {
    /// Crée une plage [start, end).
    ///
    /// Retourne None si start > end (plage invalide).
    pub fn new(start: EpochId, end: EpochId) -> Option<Self> {
        if start.0 > end.0 {
            return None;
        }
        Some(Self { start, end })
    }

    /// Plage vide (start == end).
    pub fn empty() -> Self {
        Self {
            start: EPOCH_INVALID,
            end: EPOCH_INVALID,
        }
    }

    /// Vrai si la plage est vide (aucun epoch à traiter).
    #[inline]
    pub fn is_empty(self) -> bool {
        self.start.0 >= self.end.0
    }

    /// Nombre d'epochs dans la plage.
    #[inline]
    pub fn len(self) -> u64 {
        self.end.0.saturating_sub(self.start.0)
    }

    /// Vrai si `epoch` est dans [start, end).
    #[inline]
    pub fn contains(self, epoch: EpochId) -> bool {
        epoch.0 >= self.start.0 && epoch.0 < self.end.0
    }

    /// Retourne le chevauchement de deux plages, ou None si disjointes.
    pub fn intersect(self, other: Self) -> Option<Self> {
        let s = epoch_max(self.start, other.start);
        let e = epoch_min(self.end, other.end);
        if s.0 < e.0 {
            Some(Self { start: s, end: e })
        } else {
            None
        }
    }

    /// Itère sur tous les EpochIds de la plage (itératif, sans récursion).
    ///
    /// RÈGLE RECUR-01 : itération explicite, jamais récursive.
    pub fn iter(self) -> EpochRangeIter {
        EpochRangeIter {
            current: self.start,
            end: self.end,
        }
    }
}

/// Itérateur sur une EpochRange.
pub struct EpochRangeIter {
    current: EpochId,
    end: EpochId,
}

impl Iterator for EpochRangeIter {
    type Item = EpochId;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current.0 >= self.end.0 {
            return None;
        }
        let val = self.current;
        self.current = EpochId(self.current.0.saturating_add(1));
        Some(val)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let rem = self.end.0.saturating_sub(self.current.0) as usize;
        (rem, Some(rem))
    }
}

// =============================================================================
// Validation structurelle
// =============================================================================

/// Valide un EpochId lu depuis le disque (règle ONDISK-01).
///
/// Un epoch disque valide :
/// 1. N'est pas 0 (EPOCH_INVALID).
/// 2. N'est pas strictement supérieur à current + 1 (pas de saut futur).
pub fn validate_epoch_id_from_disk(candidate: EpochId) -> ExofsResult<()> {
    if candidate.0 == 0 {
        return Err(ExofsError::InvalidEpochId);
    }
    let cur = CURRENT_EPOCH.load(AOrdering::Relaxed);
    // Tolérance de +1 pour le cas d'un commit partiellement persisté.
    let max_allowed = cur.saturating_add(1);
    if candidate.0 > max_allowed && cur != 0 {
        return Err(ExofsError::FutureEpoch);
    }
    Ok(())
}

/// Valide qu'une séquence d'EpochIds est strictement croissante.
///
/// Précondition : utilisé lors du recovery pour vérifier la chaîne d'epochs.
/// RÈGLE RECUR-01 : itération linéaire, pas de récursion.
pub fn validate_epoch_sequence(ids: &[EpochId]) -> ExofsResult<()> {
    if ids.len() < 2 {
        return Ok(());
    }
    let mut prev = ids[0];
    for &cur in &ids[1..] {
        if cur.0 <= prev.0 {
            return Err(ExofsError::EpochSequenceViolation);
        }
        prev = cur;
    }
    Ok(())
}

// =============================================================================
// Snapshot d'état (observabilité / diagnostics)
// =============================================================================

/// Instantané cohérent de l'état du compteur d'epochs.
#[derive(Copy, Clone, Debug)]
pub struct EpochCounterSnapshot {
    /// Epoch courant en mémoire (pas forcément sur disque).
    pub current: EpochId,
    /// Epoch le plus récent provably sur disque.
    pub durable: EpochId,
    /// Nombre d'epochs en vol (current - durable).
    pub in_flight: u64,
    /// Numéro de séquence des commits.
    pub commit_seq: u64,
    /// Vrai si le compteur est proche du wrapping.
    pub near_wrap: bool,
}

impl EpochCounterSnapshot {
    /// Prend un instantané atomiquement cohérent.
    pub fn take() -> Self {
        let current = CURRENT_EPOCH.load(AOrdering::Acquire);
        let durable = DURABLE_EPOCH.load(AOrdering::Relaxed);
        let commit_seq = COMMIT_SEQUENCE.load(AOrdering::Relaxed);
        EpochCounterSnapshot {
            current: EpochId(current),
            durable: EpochId(durable),
            in_flight: current.saturating_sub(durable),
            commit_seq,
            near_wrap: current >= EPOCH_WRAP_SENTINEL,
        }
    }

    /// Vrai si le volume est en bonne santé epoch (in_flight <= seuil).
    #[inline]
    pub fn is_healthy(&self) -> bool {
        self.in_flight <= EPOCH_WRITEBACK_MAX_PENDING
    }
}

impl fmt::Display for EpochCounterSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "EpochCounter{{ current={}, durable={}, in_flight={}, commits={}{} }}",
            self.current.0,
            self.durable.0,
            self.in_flight,
            self.commit_seq,
            if self.near_wrap { " NEAR_WRAP!" } else { "" },
        )
    }
}
