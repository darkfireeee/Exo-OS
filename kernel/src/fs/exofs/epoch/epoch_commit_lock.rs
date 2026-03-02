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

use crate::scheduler::sync::spinlock::SpinLock;

/// Lock global de commit Epoch.
///
/// Tenu uniquement pendant les 3 phases du protocole de commit.
/// INTERDIT d'acquérir depuis le GC thread (règle DEAD-01).
/// INTERDIT de tenir pendant une I/O disque directe (règle LOCK-05).
pub static EPOCH_COMMIT_LOCK: SpinLock<EpochCommitState> =
    SpinLock::new(EpochCommitState::new());

/// État interne du commit courant.
pub struct EpochCommitState {
    /// Numéro du commit en cours (0 = aucun commit actif).
    pub commit_seq: u64,
    /// Nombre total de commits réussis.
    pub total_commits: u64,
    /// Nombre de commits avortés (erreur I/O, etc.).
    pub aborted_commits: u64,
}

impl EpochCommitState {
    pub const fn new() -> Self {
        Self {
            commit_seq:      0,
            total_commits:   0,
            aborted_commits: 0,
        }
    }
}
