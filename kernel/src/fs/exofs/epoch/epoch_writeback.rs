// kernel/src/fs/exofs/epoch/epoch_writeback.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Thread de writeback périodique des epochs — flush automatique
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Ce module gère la politique de commit automatique :
// - Commit si delta > EPOCH_MAX_OBJECTS / 2 (mode préemptif).
// - Commit périodique toutes les N nanosecondes (configurable).
// - Commit forcé sur fsync() depuis posix_bridge.
//
// RÈGLE EPOCH-05 : commit anticipé si EpochRoot > 500 objets.
// RÈGLE EPOCH-03 : acquire EPOCH_COMMIT_LOCK avant chaque commit.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::fs::exofs::core::{ExofsError, ExofsResult, EpochId};
use crate::fs::exofs::core::config::EXOFS_CONFIG;
use crate::fs::exofs::core::stats::EXOFS_STATS;
use crate::fs::exofs::epoch::epoch_stats::EPOCH_STATS;
use crate::fs::exofs::epoch::epoch_delta::EpochDelta;
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// État du thread de writeback
// ─────────────────────────────────────────────────────────────────────────────

/// Raison d'un flush.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FlushReason {
    /// Flush périodique automatique.
    Periodic,
    /// Delta saturé (> EPOCH_MAX_OBJECTS / 2).
    DeltaFull,
    /// fsync() explicite depuis userspace.
    Explicit,
    /// Démontage du volume.
    Umount,
}

/// Contrôleur du thread de writeback.
pub struct WritebackController {
    /// Vrai si le thread est actif.
    running:         AtomicBool,
    /// Timestamp TSC du dernier commit.
    last_commit_tsc: AtomicU64,
    /// Nombre de commits périodiques effectués.
    periodic_commits: AtomicU64,
    /// Intervalle de commit en TSC-ticks (défaut : ~10ms).
    interval_ticks:  AtomicU64,
}

impl WritebackController {
    pub const fn new() -> Self {
        Self {
            running:         AtomicBool::new(false),
            last_commit_tsc: AtomicU64::new(0),
            periodic_commits: AtomicU64::new(0),
            interval_ticks:  AtomicU64::new(10_000_000), // ~10ms @ 1GHz TSC
        }
    }

    /// Lance le thread de writeback (appel unique au montage).
    pub fn start(&self) {
        self.running.store(true, Ordering::SeqCst);
    }

    /// Arrête le thread de writeback (appel au démontage).
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    /// Vrai si le thread est actif.
    #[inline]
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    /// Met à jour le timestamp du dernier commit.
    pub fn record_commit(&self, tsc_now: u64) {
        self.last_commit_tsc.store(tsc_now, Ordering::Relaxed);
        self.periodic_commits.fetch_add(1, Ordering::Relaxed);
    }

    /// Vrai si un commit périodique est nécessaire (TSC dépassé).
    pub fn needs_periodic_flush(&self, tsc_now: u64) -> bool {
        let last  = self.last_commit_tsc.load(Ordering::Relaxed);
        let interval = self.interval_ticks.load(Ordering::Relaxed);
        tsc_now.saturating_sub(last) >= interval
    }

    /// Configure l'intervalle de flush en TSC ticks.
    pub fn set_interval_ticks(&self, ticks: u64) {
        self.interval_ticks.store(ticks, Ordering::Relaxed);
    }
}

/// Contrôleur global du writeback.
pub static WRITEBACK_CTL: WritebackController = WritebackController::new();

// ─────────────────────────────────────────────────────────────────────────────
// Décision de flush
// ─────────────────────────────────────────────────────────────────────────────

/// Détermine si un flush doit être déclenché en fonction de l'état du delta.
///
/// RÈGLE EPOCH-05 : flush si delta.len() >= EPOCH_MAX_OBJECTS / 2.
pub fn should_flush_now(delta: &EpochDelta, tsc_now: u64) -> Option<FlushReason> {
    let half_max = crate::fs::exofs::core::EPOCH_MAX_OBJECTS / 2;
    if delta.len() >= half_max {
        return Some(FlushReason::DeltaFull);
    }
    if WRITEBACK_CTL.needs_periodic_flush(tsc_now) {
        return Some(FlushReason::Periodic);
    }
    None
}

/// Enregistre le résultat d'un flush pour les statistiques.
pub fn record_flush(reason: FlushReason, tsc_now: u64, object_count: u32) {
    WRITEBACK_CTL.record_commit(tsc_now);
    EPOCH_STATS.add_objects_committed(object_count as u64);
    match reason {
        FlushReason::Periodic => {}
        FlushReason::DeltaFull => { EPOCH_STATS.inc_forced_commits(); }
        FlushReason::Explicit  => {}
        FlushReason::Umount    => {}
    }
}
