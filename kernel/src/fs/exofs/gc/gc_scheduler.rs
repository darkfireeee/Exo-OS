//! Planificateur de passes GC ExoFS.
//!
//! Décide quand lancer une passe en fonction de la pression mémoire,
//! du remplissage du heap, et d'une période maximale entre passes.
//!
//! RÈGLE 13 : n'acquiert jamais EPOCH_COMMIT_LOCK.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::fs::exofs::core::FsError;
use crate::fs::exofs::gc::gc_state::GC_STATE;
use crate::fs::exofs::gc::gc_tuning::GcTuning;

/// Décision prise par le planificateur.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GcDecision {
    /// Lancer une passe GC maintenant.
    RunNow,
    /// Attendre encore `ticks` avant de réévaluer.
    Wait(u64),
    /// GC déjà en cours — ne pas relancer.
    AlreadyRunning,
}

/// État interne du planificateur.
pub struct GcScheduler {
    tuning: GcTuning,
    /// Tick de la dernière passe terminée.
    last_pass_tick: AtomicU64,
    /// Nombre de bytes alloués depuis la dernière passe.
    bytes_since_last_gc: AtomicU64,
    /// Nombre de blobs créés depuis la dernière passe.
    blobs_since_last_gc: AtomicU64,
}

impl GcScheduler {
    pub const fn new(tuning: GcTuning) -> Self {
        Self {
            tuning,
            last_pass_tick: AtomicU64::new(0),
            bytes_since_last_gc: AtomicU64::new(0),
            blobs_since_last_gc: AtomicU64::new(0),
        }
    }

    /// Notifie l'allocation d'un nouveau blob.
    pub fn notify_blob_created(&self, phys_size: u64) {
        self.blobs_since_last_gc.fetch_add(1, Ordering::Relaxed);
        self.bytes_since_last_gc.fetch_add(phys_size, Ordering::Relaxed);
    }

    /// Évalue si une passe GC doit être lancée maintenant.
    pub fn evaluate(&self, current_tick: u64) -> GcDecision {
        if GC_STATE.is_active() {
            return GcDecision::AlreadyRunning;
        }

        let last = self.last_pass_tick.load(Ordering::Acquire);
        let elapsed = current_tick.saturating_sub(last);
        let bytes = self.bytes_since_last_gc.load(Ordering::Acquire);
        let blobs = self.blobs_since_last_gc.load(Ordering::Acquire);

        // Conditions de déclenchement :
        // 1. Délai maximum écoulé.
        // 2. Pression mémoire (bytes ou blobs au-dessus du seuil).
        let force_by_time = elapsed >= self.tuning.max_ticks_between_gc;
        let pressure_bytes = bytes >= self.tuning.gc_bytes_threshold;
        let pressure_blobs = blobs >= self.tuning.gc_blobs_threshold;

        if force_by_time || pressure_bytes || pressure_blobs {
            GcDecision::RunNow
        } else {
            let ticks_until = self
                .tuning
                .max_ticks_between_gc
                .saturating_sub(elapsed);
            GcDecision::Wait(ticks_until)
        }
    }

    /// Enregistre la fin d'une passe GC.
    pub fn on_pass_complete(&self, end_tick: u64) {
        self.last_pass_tick.store(end_tick, Ordering::Release);
        self.bytes_since_last_gc.store(0, Ordering::Release);
        self.blobs_since_last_gc.store(0, Ordering::Release);
    }
}
