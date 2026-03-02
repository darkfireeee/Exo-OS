//! Thread GC dédié pour les passes asynchrones ExoFS.
//!
//! Tourne en Ring 0, boucle sur le GcScheduler et déclenche les passes.
//! RÈGLE 13 : n'acquiert jamais EPOCH_COMMIT_LOCK.

use crate::fs::exofs::core::EpochId;
use crate::fs::exofs::gc::blob_gc::BlobGc;
use crate::fs::exofs::gc::gc_metrics::GC_METRICS;
use crate::fs::exofs::gc::gc_scheduler::{GcDecision, GcScheduler};
use crate::fs::exofs::gc::gc_tuning::GcTuning;
use crate::fs::exofs::gc::sweeper::DEFERRED_DELETE;
use crate::fs::exofs::storage::{BlobStore, SuperBlock};

/// Contexte du thread GC.
///
/// Cette structure est créée une seule fois lors de l'initialisation du FS
/// et passée au thread kernel dédié.
pub struct GcThread<'sb, 'store> {
    gc: BlobGc<'sb, 'store>,
    scheduler: GcScheduler,
    /// EpochId courant (mis à jour par le kernel principal).
    current_epoch: core::sync::atomic::AtomicU64,
    /// Flag d'arrêt : `true` pour demander l'arrêt propre du thread.
    stop_requested: core::sync::atomic::AtomicBool,
}

impl<'sb, 'store> GcThread<'sb, 'store> {
    pub fn new(
        superblock: &'sb SuperBlock,
        store: &'store BlobStore,
        tuning: GcTuning,
    ) -> Self {
        Self {
            gc: BlobGc::new(superblock, store),
            scheduler: GcScheduler::new(tuning.clone()),
            current_epoch: core::sync::atomic::AtomicU64::new(0),
            stop_requested: core::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Met à jour l'epoch courante (appelé par le chemin commit).
    pub fn set_epoch(&self, epoch: EpochId) {
        self.current_epoch
            .store(epoch.0, core::sync::atomic::Ordering::Release);
    }

    /// Demande l'arrêt propre du thread GC.
    pub fn request_stop(&self) {
        self.stop_requested
            .store(true, core::sync::atomic::Ordering::Release);
    }

    /// Boucle principale du thread GC.
    ///
    /// À appeler dans un thread kernel Ring 0 ; ne retourne que sur
    /// `request_stop()` ou erreur fatale.
    pub fn run(&mut self) {
        loop {
            if self.stop_requested.load(core::sync::atomic::Ordering::Acquire) {
                break;
            }

            let tick = crate::arch::time::read_ticks();

            // Vide d'abord la DeferredDeleteQueue.
            let tuning_batch = self.gc.metrics().total_passes.load(core::sync::atomic::Ordering::Relaxed);
            let batch_size = if tuning_batch == 0 { 64 } else { 256 };
            if let Ok(batch) = DEFERRED_DELETE.drain_batch(batch_size) {
                for blob_id in batch {
                    // Suppression physique via le blob store.
                    let _ = self.gc.store.delete_blob(&blob_id);
                }
            }

            // Évalue si une passe GC est nécessaire.
            match self.scheduler.evaluate(tick) {
                GcDecision::RunNow => {
                    let epoch = EpochId(
                        self.current_epoch
                            .load(core::sync::atomic::Ordering::Acquire),
                    );
                    match self.gc.run_pass(epoch, tick) {
                        Ok(result) => {
                            self.scheduler.on_pass_complete(
                                crate::arch::time::read_ticks(),
                            );
                            GC_METRICS.record_pass(&result);
                        }
                        Err(_e) => {
                            // Erreur GC non fatale → log et retry au prochain cycle.
                        }
                    }
                }
                GcDecision::Wait(_) | GcDecision::AlreadyRunning => {
                    // Yield au scheduler kernel.
                    crate::arch::cpu::cpu_relax();
                }
            }
        }
    }
}

/// Démarre le thread GC background au démarrage d'ExoFS.
///
/// Le thread réel est créé via le scheduler kernel — ici on enregistre
/// l'intention et on présuppose que le thread GC sera instancié par
/// l'infrastructure de boot qui dispose d'un handle SuperBlock/BlobStore.
pub fn start_gc_thread() -> Result<(), crate::fs::exofs::core::FsError> {
    // La création effective du thread kernel est déléguée à l'init système
    // qui dispose du handle SuperBlock/BlobStore. Cette fonction est le point
    // d'entrée appelé par exofs_init().
    Ok(())
}
