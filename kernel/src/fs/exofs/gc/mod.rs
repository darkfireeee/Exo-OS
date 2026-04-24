// kernel/src/fs/exofs/gc/mod.rs
//
// ==============================================================================
// Module GC ExoFS — Garbage Collector Tricolore
// Ring 0 . no_std . Exo-OS
//
// Ce module expose l'API publique du GC ExoFS : collecteur tricolore avec
// phases Scan / Mark / Sweep / Orphan / Inline / Finalize.
//
// Architecture :
//   gc_state         — État de phase (Idle/Scanning/Marking/Sweeping/Finalizing)
//   tricolor         — Workspace tricolore (White/Grey/Black) + file grise bornée
//   blob_refcount    — Compteur de références atomique + file de suppression différée
//   gc_metrics       — Métriques atomiques (compteurs, histogrammes de phase)
//   gc_tuning        — Auto-tuning du GC (seuils, aggressivité)
//   reference_tracker — Graphe objet→blobs et blob→sous-blobs
//   relation_walker  — Traversée du graphe de Relations (GC-02)
//   epoch_scanner    — Scan des EpochRoots A/B/C (GC-06)
//   marker           — Phase de marquage tricolore
//   sweeper          — Phase de balayage (collecte blobs blancs)
//   cycle_detector   — Détection de cycles DFS itératif
//   orphan_collector — Collecte des objets/blobs orphelins
//   inline_gc        — GC des objets à stockage inline (< 512 B)
//   blob_gc          — Orchestrateur principal (6 phases)
//   gc_scheduler     — Planificateur GC non-bloquant
//   gc_thread        — Thread de fond GC (boucle run())
//
// Conformite spec ExoFS_Reference_Complete_v2.md :
//   GC-01 à GC-09, REFCNT-01, RECUR-01, DEAD-01, DAG-01, RACE-01, ARITH-02, OOM-02
// ==============================================================================

// ── Déclarations de sous-modules ────────────────────────────────────────────

pub mod blob_gc;
pub mod blob_refcount;
pub mod cycle_detector;
pub mod epoch_scanner;
pub mod gc_metrics;
pub mod gc_scheduler;
pub mod gc_state;
pub mod gc_thread;
pub mod gc_tuning;
pub mod inline_gc;
pub mod marker;
pub mod orphan_collector;
pub mod reference_tracker;
pub mod relation_walker;
pub mod sweeper;
pub mod tricolor;

// ── Re-exports : API publique GC ────────────────────────────────────────────

// gc_state
pub use gc_state::{GcPassStats, GcPhase, GcStateSnapshot, GC_STATE};

// tricolor
pub use tricolor::{
    BlobNode, MarkStats, SweepResult, TriColor, TricolorWorkspace, GC_MARK_BATCH_SIZE,
    MAX_GC_GREY_QUEUE,
};

// blob_refcount
pub use blob_refcount::{BLOB_REFCOUNT, GC_MIN_DEFERRED_EPOCHS};

// gc_metrics
pub use gc_metrics::{GcMetricsSnapshot, GC_METRICS};

// gc_tuning
pub use gc_tuning::{GcSystemState, GcTriggerReason, GcTuningParams, GC_TUNER};

// reference_tracker
pub use reference_tracker::{ReferenceTracker, REFERENCE_TRACKER};

// relation_walker
pub use relation_walker::{RelationEdge, WalkStats, RELATION_WALKER};

// epoch_scanner
pub use epoch_scanner::{BlobLookup, EmptyBlobLookup, EpochScanSnapshot, ScanStats, EPOCH_SCANNER};

// marker
pub use marker::{MarkerConfig, MarkingResult, MARKER};

// sweeper
pub use sweeper::{SweepConfig, SweeperResult, SWEEPER};

// cycle_detector
pub use cycle_detector::{CycleDetectStats, DetectedCycle, CYCLE_DETECTOR};

// orphan_collector
pub use orphan_collector::{OrphanResult, ORPHAN_COLLECTOR};

// inline_gc
pub use inline_gc::{
    InlineGcResult, InlineGcStats, InlineObjectEntry, INLINE_DATA_THRESHOLD, INLINE_GC,
};

// blob_gc
pub use blob_gc::{BlobGcConfig, GcPassResult, BLOB_GC};

// gc_scheduler
pub use gc_scheduler::{GcSchedulerStats, ScheduleDecision, ScheduleReason, GC_SCHEDULER};

// gc_thread
pub use gc_thread::{gc_thread_entry, GcThread, GcThreadControl, GcThreadStats, GC_THREAD};

// ==============================================================================
// Initialisation du sous-système GC
// ==============================================================================

/// Initialise le sous-système GC ExoFS.
///
/// A appeler au demarrage du kernel, avant de lancer `gc_thread_entry()`.
///
/// Cette fonction :
/// 1. Valide les paramètres de tuning via GC_TUNER
/// 2. Active le scheduler
/// 3. Enregistre bootstrap dans le scheduler
///
/// Thread-safe : utilise uniquement les singletons atomiques.
pub fn gc_init() {
    // Activer le scheduler GC.
    GC_SCHEDULER.set_enabled(true);

    // Validation du tuner.
    GC_TUNER.validate_params().ok();

    // Signaler le demarrage initial.
    GC_SCHEDULER.force_trigger(ScheduleReason::Bootstrap);
}

/// Arrête proprement le thread GC.
///
/// Appeler avant le démontage du système de fichiers.
pub fn gc_shutdown() {
    GC_THREAD.shutdown();
    GC_SCHEDULER.set_enabled(false);
}

/// Demande une passe GC urgente (expose par le syscall 514).
///
/// GC-05 : operation non-bloquante.
pub fn gc_force_pass() {
    GC_THREAD.trigger_urgent();
}

/// Retourne un instantane des statistiques globales du GC.
pub fn gc_metrics_snapshot() -> GcMetricsSnapshot {
    GC_METRICS.snapshot()
}

/// Retourne l'état courant du GC.
pub fn gc_state_snapshot() -> GcStateSnapshot {
    GC_STATE.snapshot()
}
