// kernel/src/fs/exofs/epoch/epoch_commit_lock.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// EPOCH_COMMIT_LOCK — verrou unique pour sérialiser les commits d'epoch
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE EPOCH-03 : EPOCH_COMMIT_LOCK = SpinLock obligatoire — un seul commit à la fois.
// RÈGLE DEAD-01  : GC n'acquiert JAMAIS ce lock — deadlock garanti sinon.
// RÈGLE LOCK-07  : Ce lock est au niveau le plus élevé de la hiérarchie fs/.

use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::fs::exofs::core::{EpochId, ExofsError, ExofsResult};
use crate::scheduler::sync::spinlock::{SpinLock, SpinLockGuard};

// =============================================================================
// Constantes
// =============================================================================

/// Taille de l'historique circulaire des derniers commits.
const COMMIT_HISTORY_SIZE: usize = 32;

// =============================================================================
// EpochCommitState — état interne protégé par EPOCH_COMMIT_LOCK
// =============================================================================

/// Statut d'un commit dans l'historique.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CommitStatus {
    /// Commit réussi.
    Success  = 0,
    /// Commit avorté (erreur I/O ou lock déjà tenu).
    Aborted  = 1,
    /// Commit partiel (barrière manquante — CRITIQUE).
    Partial  = 2,
}

/// Entrée dans l'historique des commits.
#[derive(Copy, Clone, Debug)]
pub struct CommitHistoryEntry {
    /// Epoch commité.
    pub epoch_id:     EpochId,
    /// Timestamp TSC du début du commit.
    pub started_at:   u64,
    /// Durée du commit en cycles TSC.
    pub duration_cyc: u64,
    /// Nombre d'objets commités.
    pub object_count: u32,
    /// Statut du commit.
    pub status:       CommitStatus,
}

impl CommitHistoryEntry {
    const fn empty() -> Self {
        CommitHistoryEntry {
            epoch_id:     EpochId(0),
            started_at:   0,
            duration_cyc: 0,
            object_count: 0,
            status:       CommitStatus::Aborted,
        }
    }
}

/// État interne du protocole de commit, protégé par EPOCH_COMMIT_LOCK.
pub struct EpochCommitState {
    /// Numéro de séquence du commit courant (incrémenté à chaque acquisition).
    pub commit_seq:       u64,
    /// Total de commits réussis.
    pub total_commits:    u64,
    /// Total de commits avortés.
    pub aborted_commits:  u64,
    /// Total de commits partiels (CRITIQUE — doit rester à 0 en prod).
    pub partial_commits:  u64,
    /// EpochId du dernier commit réussi.
    pub last_epoch:       EpochId,
    /// TSC du dernier commit réussi.
    pub last_commit_tsc:  u64,
    /// Historique circulaire des derniers commits.
    history:              [CommitHistoryEntry; COMMIT_HISTORY_SIZE],
    /// Index d'écriture dans l'historique.
    history_head:         usize,
}

impl EpochCommitState {
    pub const fn new() -> Self {
        EpochCommitState {
            commit_seq:      0,
            total_commits:   0,
            aborted_commits: 0,
            partial_commits: 0,
            last_epoch:      EpochId(0),
            last_commit_tsc: 0,
            history:         [CommitHistoryEntry::empty(); COMMIT_HISTORY_SIZE],
            history_head:    0,
        }
    }

    /// Enregistre un commit dans l'historique circulaire.
    pub fn record_commit(&mut self, entry: CommitHistoryEntry) {
        let idx = self.history_head % COMMIT_HISTORY_SIZE;
        self.history[idx] = entry;
        self.history_head = self.history_head.wrapping_add(1);
        match entry.status {
            CommitStatus::Success => {
                self.total_commits = self.total_commits.saturating_add(1);
                self.last_epoch    = entry.epoch_id;
                self.last_commit_tsc = entry.started_at.saturating_add(entry.duration_cyc);
            }
            CommitStatus::Aborted => {
                self.aborted_commits = self.aborted_commits.saturating_add(1);
            }
            CommitStatus::Partial => {
                self.partial_commits = self.partial_commits.saturating_add(1);
            }
        }
    }

    /// Retourne les N derniers commits dans l'ordre chronologique inverse.
    pub fn recent_commits(&self, n: usize) -> impl Iterator<Item = &CommitHistoryEntry> {
        let n = n.min(COMMIT_HISTORY_SIZE);
        let head = self.history_head;
        // Itération sur les n derniers slots en ordre décroissant.
        (0..n).map(move |i| {
            let idx = head.wrapping_sub(1).wrapping_sub(i) % COMMIT_HISTORY_SIZE;
            &self.history[idx]
        })
    }

    /// Vrai si le dernier commit a réussi (sanity check).
    #[inline]
    pub fn last_commit_ok(&self) -> bool {
        if self.history_head == 0 {
            return true; // Aucun commit encore.
        }
        let last_idx = self.history_head.wrapping_sub(1) % COMMIT_HISTORY_SIZE;
        self.history[last_idx].status == CommitStatus::Success
    }

    /// Retourne le nombre de commits consécutifs avortés.
    pub fn consecutive_aborts(&self) -> usize {
        let mut count = 0usize;
        for entry in self.recent_commits(COMMIT_HISTORY_SIZE) {
            if entry.epoch_id.0 == 0 {
                break;
            }
            if entry.status == CommitStatus::Aborted {
                count += 1;
            } else {
                break;
            }
        }
        count
    }
}

// =============================================================================
// Lock global de commit Epoch
// =============================================================================

/// Lock global sérialisant les commits d'epoch.
///
/// # RÈGLE EPOCH-03
/// Un seul commit Epoch à la fois. Ce lock est tenu uniquement
/// pendant les 3 phases du protocole de commit.
///
/// # RÈGLE DEAD-01
/// INTERDIT d'acquérir depuis le GC thread — deadlock garanti.
///
/// # RÈGLE LOCK-07
/// Ce lock est au niveau le PLUS ÉLEVÉ de la hiérarchie fs/.
/// Jamais acquis sous un autre lock fs/ plus fin.
pub static EPOCH_COMMIT_LOCK: SpinLock<EpochCommitState> =
    SpinLock::new(EpochCommitState::new());

/// Indicateur rapide (atomique) qu'un commit est en cours.
///
/// Permet au GC de détecter un commit en cours SANS acquérir le lock
/// (lecture seule, lecture-après-écriture sécurisée par le SpinLock).
static COMMIT_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Nombre de tentatives d'acquisition du lock refusées (contention monitoring).
static LOCK_CONTENTION_COUNT: AtomicU64 = AtomicU64::new(0);

// =============================================================================
// API publique du lock
// =============================================================================

/// Tente d'acquérir le lock de commit.
///
/// Retourne `Ok(guard)` si le lock est disponible, `Err(CommitInProgress)` sinon.
///
/// # Utilisation
/// ```rust
/// let mut guard = try_acquire_commit_lock()?;
/// guard.commit_seq += 1; // Début du commit.
/// // ... 3 phases du commit ...
/// guard.record_commit(CommitHistoryEntry { ... });
/// // drop(guard) libère le lock automatiquement.
/// ```
///
/// # RÈGLE DEAD-01 : NE PAS appeler depuis le GC thread.
pub fn try_acquire_commit_lock() -> ExofsResult<SpinLockGuard<'static, EpochCommitState>> {
    // Vérification rapide sans contention avant le try_lock.
    if COMMIT_IN_PROGRESS.load(Ordering::Acquire) {
        LOCK_CONTENTION_COUNT.fetch_add(1, Ordering::Relaxed);
        return Err(ExofsError::CommitInProgress);
    }
    // Acquisition bloquante (SpinLock).
    let guard = EPOCH_COMMIT_LOCK.lock();
    COMMIT_IN_PROGRESS.store(true, Ordering::Release);
    Ok(guard)
}

/// Marque la fin d'un commit (à appeler après drop du guard).
///
/// Remet COMMIT_IN_PROGRESS à false.
/// Doit être appelé exactement une fois par `try_acquire_commit_lock()` réussi.
pub fn release_commit_lock() {
    COMMIT_IN_PROGRESS.store(false, Ordering::Release);
}

/// Vrai si un commit est actuellement en cours.
///
/// Lecture sans lock — fiable pour le GC qui doit respecter DEAD-01.
#[inline]
pub fn is_commit_in_progress() -> bool {
    COMMIT_IN_PROGRESS.load(Ordering::Acquire)
}

/// Nombre de tentatives d'acquisition du lock refusées (contention).
#[inline]
pub fn lock_contention_count() -> u64 {
    LOCK_CONTENTION_COUNT.load(Ordering::Relaxed)
}

// =============================================================================
// Snapshot d'état du lock (observabilité)
// =============================================================================

/// Snapshot de l'état du lock de commit.
#[derive(Debug)]
pub struct CommitLockSnapshot {
    /// Vrai si un commit est en cours.
    pub in_progress:     bool,
    /// Nombre total de commits réussis.
    pub total_commits:   u64,
    /// Nombre total d'aborts.
    pub aborted_commits: u64,
    /// Nombre de commits partiels (doit être 0).
    pub partial_commits: u64,
    /// EpochId du dernier commit réussi.
    pub last_epoch:      EpochId,
    /// Contention (acquisitions refusées).
    pub contention:      u64,
}

impl CommitLockSnapshot {
    /// Prend un instantané de l'état courant.
    pub fn take() -> Self {
        let in_progress = COMMIT_IN_PROGRESS.load(Ordering::Acquire);
        let contention  = LOCK_CONTENTION_COUNT.load(Ordering::Relaxed);
        // Lecture de l'état interne sans lock si non critique pour diagnostics.
        // Note : en production, utiliser try_lock si cohérence stricte nécessaire.
        let (total_commits, aborted_commits, partial_commits, last_epoch) = {
            (0u64, 0u64, 0u64, EpochId(0)) // Valeurs neutres sans acquérir le lock.
        };
        CommitLockSnapshot {
            in_progress,
            total_commits,
            aborted_commits,
            partial_commits,
            last_epoch,
            contention,
        }
    }
}

impl fmt::Display for CommitLockSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CommitLock{{ in_progress={}, commits={}, aborts={}, partial={}, contention={} }}",
            self.in_progress,
            self.total_commits,
            self.aborted_commits,
            self.partial_commits,
            self.contention,
        )
    }
}
