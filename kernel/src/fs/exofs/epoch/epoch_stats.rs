// kernel/src/fs/exofs/epoch/epoch_stats.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Compteurs statistiques spécifiques à l'epoch manager
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, Ordering};

/// Compteurs atomiques de l'epoch manager.
///
/// Ces compteurs sont séparés de ExofsStats pour permettre une réinitialisation
/// indépendante et un affichage dédié dans les outils de diagnostic.
pub struct EpochStats {
    /// Nombre d'epochs committés avec succès.
    pub commits_ok:               AtomicU64,
    /// Nombre d'epochs avortés (erreur I/O ou lock contention).
    pub commits_aborted:          AtomicU64,
    /// Nombre de commits anticipés (EpochRoot > EPOCH_MAX_OBJECTS).
    pub forced_commits:           AtomicU64,
    /// Nombre de récupérations slot dégradé (< 3 slots valides).
    pub degraded_recoveries:      AtomicU64,
    /// Nombre de barrières Phase 1 exécutées.
    pub barriers_data:            AtomicU64,
    /// Nombre de barrières Phase 2 exécutées.
    pub barriers_root:            AtomicU64,
    /// Nombre de barrières Phase 3 exécutées.
    pub barriers_record:          AtomicU64,
    /// Nombre total d'objets modifiés committés.
    pub objects_committed:        AtomicU64,
    /// Nombre total d'objets supprimés committés.
    pub objects_deleted_committed: AtomicU64,
    /// Nombre d'époques où une page EpochRoot chaîné a été écrite (> 1 page).
    pub chained_root_pages:       AtomicU64,
    /// Temps cumulé de commit (en nanosecondes, estimation via TSC).
    pub commit_time_ns_total:     AtomicU64,
}

impl EpochStats {
    const fn new() -> Self {
        macro_rules! z { () => { AtomicU64::new(0) } }
        Self {
            commits_ok:               z!(),
            commits_aborted:          z!(),
            forced_commits:           z!(),
            degraded_recoveries:      z!(),
            barriers_data:            z!(),
            barriers_root:            z!(),
            barriers_record:          z!(),
            objects_committed:        z!(),
            objects_deleted_committed: z!(),
            chained_root_pages:       z!(),
            commit_time_ns_total:     z!(),
        }
    }

    #[inline] pub fn inc_commits_ok(&self)                  { self.commits_ok.fetch_add(1, Ordering::Relaxed); }
    #[inline] pub fn inc_commits_aborted(&self)             { self.commits_aborted.fetch_add(1, Ordering::Relaxed); }
    #[inline] pub fn inc_forced_commits(&self)              { self.forced_commits.fetch_add(1, Ordering::Relaxed); }
    #[inline] pub fn inc_degraded_recoveries(&self)         { self.degraded_recoveries.fetch_add(1, Ordering::Relaxed); }
    #[inline] pub fn inc_barriers_data(&self)               { self.barriers_data.fetch_add(1, Ordering::Relaxed); }
    #[inline] pub fn inc_barriers_root(&self)               { self.barriers_root.fetch_add(1, Ordering::Relaxed); }
    #[inline] pub fn inc_barriers_record(&self)             { self.barriers_record.fetch_add(1, Ordering::Relaxed); }
    #[inline] pub fn add_objects_committed(&self, n: u64)   { self.objects_committed.fetch_add(n, Ordering::Relaxed); }
    #[inline] pub fn add_objects_deleted(&self, n: u64)     { self.objects_deleted_committed.fetch_add(n, Ordering::Relaxed); }
    #[inline] pub fn inc_chained_root_pages(&self)          { self.chained_root_pages.fetch_add(1, Ordering::Relaxed); }
    #[inline] pub fn add_commit_time_ns(&self, ns: u64)     { self.commit_time_ns_total.fetch_add(ns, Ordering::Relaxed); }

    /// Snapshot des compteurs pour affichage (lecture non-atomique cohérente).
    pub fn snapshot(&self) -> EpochStatsSnapshot {
        EpochStatsSnapshot {
            commits_ok:               self.commits_ok.load(Ordering::Relaxed),
            commits_aborted:          self.commits_aborted.load(Ordering::Relaxed),
            forced_commits:           self.forced_commits.load(Ordering::Relaxed),
            degraded_recoveries:      self.degraded_recoveries.load(Ordering::Relaxed),
            barriers_data:            self.barriers_data.load(Ordering::Relaxed),
            barriers_root:            self.barriers_root.load(Ordering::Relaxed),
            barriers_record:          self.barriers_record.load(Ordering::Relaxed),
            objects_committed:        self.objects_committed.load(Ordering::Relaxed),
            objects_deleted_committed: self.objects_deleted_committed.load(Ordering::Relaxed),
            chained_root_pages:       self.chained_root_pages.load(Ordering::Relaxed),
            commit_time_ns_total:     self.commit_time_ns_total.load(Ordering::Relaxed),
        }
    }
}

/// Vue instantanée non-atomique des compteurs (pour logging/ioctl).
#[derive(Copy, Clone, Debug)]
pub struct EpochStatsSnapshot {
    pub commits_ok:               u64,
    pub commits_aborted:          u64,
    pub forced_commits:           u64,
    pub degraded_recoveries:      u64,
    pub barriers_data:            u64,
    pub barriers_root:            u64,
    pub barriers_record:          u64,
    pub objects_committed:        u64,
    pub objects_deleted_committed: u64,
    pub chained_root_pages:       u64,
    pub commit_time_ns_total:     u64,
}

/// Singleton global des statistiques epoch.
pub static EPOCH_STATS: EpochStats = EpochStats::new();
