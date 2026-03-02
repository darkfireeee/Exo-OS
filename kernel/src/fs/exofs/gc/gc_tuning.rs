//! Paramètres de tuning du Garbage Collector ExoFS.

/// Paramètres configurables du GC.
#[derive(Debug, Clone)]
pub struct GcTuning {
    /// Nombre max de ticks entre deux passes automatiques.
    pub max_ticks_between_gc: u64,
    /// Seuil de bytes alloués depuis la dernière passe pour déclencher le GC.
    pub gc_bytes_threshold: u64,
    /// Seuil de blobs créés depuis la dernière passe.
    pub gc_blobs_threshold: u64,
    /// Taille d'un lot de suppressions différées traitées en une itération.
    pub deferred_delete_batch_size: usize,
    /// Délai maximum (ticks) entre deux traitements de la DeferredDeleteQueue.
    pub deferred_flush_interval_ticks: u64,
    /// Activer le GC inline (inline_gc) sur les chemins de write.
    pub inline_gc_enabled: bool,
    /// Seuil de ref_count de blob pour déclencher un GC inline immédiat.
    pub inline_gc_ref_threshold: u32,
}

impl GcTuning {
    /// Paramètres par défaut adaptés à un système 64-bit avec ~1 GiB de blob store.
    pub const fn default_1gib() -> Self {
        Self {
            max_ticks_between_gc: 10_000_000, // ~10 s @ 1 MHz
            gc_bytes_threshold: 128 * 1024 * 1024, // 128 MiB
            gc_blobs_threshold: 65_536,
            deferred_delete_batch_size: 256,
            deferred_flush_interval_ticks: 1_000_000,
            inline_gc_enabled: true,
            inline_gc_ref_threshold: 0,
        }
    }

    /// Paramètres agressifs pour tests ou environnements avec peu de RAM.
    pub const fn aggressive() -> Self {
        Self {
            max_ticks_between_gc: 1_000_000,
            gc_bytes_threshold: 16 * 1024 * 1024,
            gc_blobs_threshold: 4_096,
            deferred_delete_batch_size: 64,
            deferred_flush_interval_ticks: 100_000,
            inline_gc_enabled: true,
            inline_gc_ref_threshold: 0,
        }
    }
}
