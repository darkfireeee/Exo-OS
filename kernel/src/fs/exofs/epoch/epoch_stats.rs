// kernel/src/fs/exofs/epoch/epoch_stats.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Compteurs statistiques spécifiques à l'epoch manager
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════

use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};

// =============================================================================
// compteurs d'erreurs recovery (ajouts nécessaires aux autres modules)
// =============================================================================

/// Compteurs spécialisés pour les erreurs recovery (utilisés par epoch_recovery.rs).
pub struct EpochRecoveryStats {
    pub slot_io_errors: AtomicU64,
    pub checksum_errors: AtomicU64,
    pub degraded_mounts: AtomicU64,
    pub slot_magic_errors: AtomicU64,
    pub epochs_replayed: AtomicU64,
}

impl EpochRecoveryStats {
    const fn new() -> Self {
        macro_rules! z {
            () => {
                AtomicU64::new(0)
            };
        }
        Self {
            slot_io_errors: z!(),
            checksum_errors: z!(),
            degraded_mounts: z!(),
            slot_magic_errors: z!(),
            epochs_replayed: z!(),
        }
    }
    #[inline]
    pub fn inc_slot_io_errors(&self) {
        self.slot_io_errors.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_checksum_errors(&self) {
        self.checksum_errors.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_degraded_mounts(&self) {
        self.degraded_mounts.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_slot_magic_errors(&self) {
        self.slot_magic_errors.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_epochs_replayed(&self) {
        self.epochs_replayed.fetch_add(1, Ordering::Relaxed);
    }
}

/// Snapshot non-atomique des statistiques de recovery epoch.
#[derive(Clone, Copy, Debug, Default)]
pub struct EpochRecoveryStatsSnapshot {
    pub slot_io_errors: u64,
    pub checksum_errors: u64,
    pub degraded_mounts: u64,
    pub slot_magic_errors: u64,
    pub epochs_replayed: u64,
}

// =============================================================================
// Histogramme de latence de commit (buckets logarithmiques)
// =============================================================================

/// Nombre de buckets du l'histogramme de latence.
const LATENCY_BUCKETS: usize = 16;

/// Bornes supérieures des buckets (en cycles TSC). Dernier = overflow.
/// Buckets : < 1K, < 2K, < 4K, ..., < 32M, overflow.
const LATENCY_BOUNDS_CYCLES: [u64; LATENCY_BUCKETS - 1] = [
    1_000, 2_000, 4_000, 8_000, 16_000, 32_000, 64_000, 128_000, 256_000, 512_000, 1_000_000,
    2_000_000, 4_000_000, 8_000_000, 16_000_000,
];

/// Histogramme de latence des commits Epoch (en cycles TSC).
pub struct LatencyHistogram {
    /// Compteur par bucket.
    buckets: [AtomicU64; LATENCY_BUCKETS],
    /// Total de cycles accumulés.
    total_cycles: AtomicU64,
    /// Nombre d'échantillons enregistrés.
    count: AtomicU64,
    /// Minimum observé.
    min_cycles: AtomicU64,
    /// Maximum observé.
    max_cycles: AtomicU64,
}

impl LatencyHistogram {
    const fn new() -> Self {
        macro_rules! z {
            () => {
                AtomicU64::new(0)
            };
        }
        LatencyHistogram {
            buckets: [
                z!(),
                z!(),
                z!(),
                z!(),
                z!(),
                z!(),
                z!(),
                z!(),
                z!(),
                z!(),
                z!(),
                z!(),
                z!(),
                z!(),
                z!(),
                z!(),
            ],
            total_cycles: z!(),
            count: z!(),
            min_cycles: AtomicU64::new(u64::MAX),
            max_cycles: z!(),
        }
    }

    /// Enregistre une mesure de latence (en cycles TSC).
    pub fn record(&self, cycles: u64) {
        // Sélection du bucket.
        let bidx = LATENCY_BOUNDS_CYCLES
            .iter()
            .position(|&bound| cycles < bound)
            .unwrap_or(LATENCY_BUCKETS - 1);
        self.buckets[bidx].fetch_add(1, Ordering::Relaxed);
        self.total_cycles.fetch_add(cycles, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
        // Mise à jour min (compare_exchange loop).
        let _ = self
            .min_cycles
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
                if cycles < old {
                    Some(cycles)
                } else {
                    None
                }
            });
        // Mise à jour max.
        let _ = self
            .max_cycles
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
                if cycles > old {
                    Some(cycles)
                } else {
                    None
                }
            });
    }

    /// Retourne un snapshot de l'histogramme.
    pub fn snapshot(&self) -> LatencyHistogramSnapshot {
        let count = self.count.load(Ordering::Relaxed);
        let total = self.total_cycles.load(Ordering::Relaxed);
        let min_obs = self.min_cycles.load(Ordering::Relaxed);
        let max_obs = self.max_cycles.load(Ordering::Relaxed);
        let mut buckets = [0u64; LATENCY_BUCKETS];
        for i in 0..LATENCY_BUCKETS {
            buckets[i] = self.buckets[i].load(Ordering::Relaxed);
        }
        LatencyHistogramSnapshot {
            buckets,
            count,
            avg_cycles: if count > 0 { total / count } else { 0 },
            min_cycles: if min_obs == u64::MAX { 0 } else { min_obs },
            max_cycles: max_obs,
        }
    }
}

/// Snapshot d'un histogramme de latence.
#[derive(Copy, Clone, Debug)]
pub struct LatencyHistogramSnapshot {
    /// Compteurs par bucket.
    pub buckets: [u64; LATENCY_BUCKETS],
    /// Nombre total d'échantillons.
    pub count: u64,
    /// Latence moyenne (cycles).
    pub avg_cycles: u64,
    /// Latence minimum observée (cycles).
    pub min_cycles: u64,
    /// Latence maximum observée (cycles).
    pub max_cycles: u64,
}

impl LatencyHistogramSnapshot {
    /// Retourne le percentile P50 approximatif (cycles).
    pub fn p50_cycles(&self) -> u64 {
        self.percentile_cycles(50)
    }

    /// Retourne le percentile P99 approximatif (cycles).
    pub fn p99_cycles(&self) -> u64 {
        self.percentile_cycles(99)
    }

    /// Calcule un percentile approx (0..100) par interpolation de bucket.
    pub fn percentile_cycles(&self, pct: u64) -> u64 {
        if self.count == 0 {
            return 0;
        }
        let target = self.count.saturating_mul(pct) / 100;
        let mut cum = 0u64;
        for (i, &cnt) in self.buckets.iter().enumerate() {
            cum = cum.saturating_add(cnt);
            if cum >= target {
                // Retourne la borne supérieure du bucket.
                if i < LATENCY_BOUNDS_CYCLES.len() {
                    return LATENCY_BOUNDS_CYCLES[i];
                } else {
                    return self.max_cycles;
                }
            }
        }
        self.max_cycles
    }
}

// =============================================================================
// EpochStats — compteurs principaux de l'epoch manager
// =============================================================================

/// Compteurs atomiques de l'epoch manager.
///
/// RÈGLE : tous les compteurs sont AtomicU64 — jamais dans une struct on-disk.
pub struct EpochStats {
    // ── Commits ──────────────────────────────────────────────────────────────
    /// Nombre d'epochs committés avec succès.
    pub commits_ok: AtomicU64,
    /// Nombre d'epochs avortés (erreur I/O ou lock contention).
    pub commits_aborted: AtomicU64,
    /// Nombre de commits forcés anticipés (EpochRoot > EPOCH_MAX_OBJECTS).
    pub forced_commits: AtomicU64,
    /// Commits ayant eu un partial (barrière manquante — CRITIQUE).
    pub partial_commits: AtomicU64,

    // ── Barrières NVMe ────────────────────────────────────────────────────
    /// Barrières Phase 1 (après payload).
    pub barriers_data: AtomicU64,
    /// Barrières Phase 2 (après EpochRoot).
    pub barriers_root: AtomicU64,
    /// Barrières Phase 3 (après EpochRecord).
    pub barriers_record: AtomicU64,
    /// Barrières ayant échoué (total des 3 phases).
    pub barrier_failures: AtomicU64,

    // ── Objets ────────────────────────────────────────────────────────────
    /// Total d'objets modifiés committés.
    pub objects_committed: AtomicU64,
    /// Total d'objets supprimés committés.
    pub objects_deleted: AtomicU64,
    /// Total d'objets créés committés.
    pub objects_created: AtomicU64,

    // ── EpochRoot ─────────────────────────────────────────────────────────
    /// Pages EpochRoot chaînées (cas multi-pages).
    pub chained_root_pages: AtomicU64,
    /// EpochRoots avec des suppressions (has_deletions).
    pub roots_with_deletions: AtomicU64,
    /// Pages EpochRoot dont la vérification a échoué.
    pub root_page_errors: AtomicU64,

    // ── Recovery ─────────────────────────────────────────────────────────
    /// Compteurs de recovery (réutilisés depuis EpochRecoveryStats).
    pub recovery: EpochRecoveryStats,

    // ── Pins ──────────────────────────────────────────────────────────────
    /// Pics max de pins simultanés.
    pub pin_max_concurrent: AtomicU64,
    /// Total de pins acquis.
    pub pins_acquired: AtomicU64,
    /// Échecs d'acquisition de pin (TooManyPins).
    pub pins_failed: AtomicU64,

    // ── Snapshots ─────────────────────────────────────────────────────────
    /// Snapshots créés.
    pub snapshots_created: AtomicU64,
    /// Snapshots supprimés.
    pub snapshots_deleted: AtomicU64,

    // ── Writeback ─────────────────────────────────────────────────────────
    /// Flushes writeback déclenchés par timer.
    pub writeback_timer_flushes: AtomicU64,
    /// Flushes writeback déclenchés par pression mémoire.
    pub writeback_pressure_flushes: AtomicU64,
    /// Flushes writeback déclenchés par EpochRoot plein.
    pub writeback_full_flushes: AtomicU64,

    // ── GC ───────────────────────────────────────────────────────────────
    /// Epochs collectés par le GC.
    pub gc_epochs_collected: AtomicU64,
    /// Objets supprimés par le GC.
    pub gc_objects_freed: AtomicU64,
    /// Cycles GC déclenchés.
    pub gc_cycles: AtomicU64,

    // ── Histogramme de latence ────────────────────────────────────────────
    /// Histogramme de latence des commits (cycles TSC).
    pub commit_latency: LatencyHistogram,
}

impl EpochStats {
    pub const fn new() -> Self {
        macro_rules! z {
            () => {
                AtomicU64::new(0)
            };
        }
        EpochStats {
            commits_ok: z!(),
            commits_aborted: z!(),
            forced_commits: z!(),
            partial_commits: z!(),
            barriers_data: z!(),
            barriers_root: z!(),
            barriers_record: z!(),
            barrier_failures: z!(),
            objects_committed: z!(),
            objects_deleted: z!(),
            objects_created: z!(),
            chained_root_pages: z!(),
            roots_with_deletions: z!(),
            root_page_errors: z!(),
            recovery: EpochRecoveryStats::new(),
            pin_max_concurrent: z!(),
            pins_acquired: z!(),
            pins_failed: z!(),
            snapshots_created: z!(),
            snapshots_deleted: z!(),
            writeback_timer_flushes: z!(),
            writeback_pressure_flushes: z!(),
            writeback_full_flushes: z!(),
            gc_epochs_collected: z!(),
            gc_objects_freed: z!(),
            gc_cycles: z!(),
            commit_latency: LatencyHistogram::new(),
        }
    }

    // ── Increments commits ─────────────────────────────────────────────────
    #[inline]
    pub fn inc_commits_ok(&self) {
        self.commits_ok.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_commits_aborted(&self) {
        self.commits_aborted.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_forced_commits(&self) {
        self.forced_commits.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_partial_commits(&self) {
        self.partial_commits.fetch_add(1, Ordering::Relaxed);
    }

    // ── Increments barrières ───────────────────────────────────────────────
    #[inline]
    pub fn inc_barriers_data(&self) {
        self.barriers_data.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_barriers_root(&self) {
        self.barriers_root.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_barriers_record(&self) {
        self.barriers_record.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_barrier_failures(&self) {
        self.barrier_failures.fetch_add(1, Ordering::Relaxed);
    }

    // ── Increments objets ──────────────────────────────────────────────────
    #[inline]
    pub fn add_objects_committed(&self, n: u64) {
        self.objects_committed.fetch_add(n, Ordering::Relaxed);
    }
    #[inline]
    pub fn add_objects_deleted(&self, n: u64) {
        self.objects_deleted.fetch_add(n, Ordering::Relaxed);
    }
    #[inline]
    pub fn add_objects_created(&self, n: u64) {
        self.objects_created.fetch_add(n, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_objects_created(&self) {
        self.objects_created.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_blobs_gc_eligible(&self) {
        self.gc_objects_freed.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_objects_read(&self) { /* no dedicated counter */
    }

    // ── Increments EpochRoot ───────────────────────────────────────────────
    #[inline]
    pub fn inc_chained_root_pages(&self) {
        self.chained_root_pages.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_roots_with_deletions(&self) {
        self.roots_with_deletions.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_root_page_errors(&self) {
        self.root_page_errors.fetch_add(1, Ordering::Relaxed);
    }

    // ── Increments recovery ────────────────────────────────────────────────
    #[inline]
    pub fn inc_recovery_slot_io_errors(&self) {
        self.recovery.inc_slot_io_errors();
    }
    #[inline]
    pub fn inc_recovery_checksum_errors(&self) {
        self.recovery.inc_checksum_errors();
    }
    #[inline]
    pub fn inc_recovery_degraded_mounts(&self) {
        self.recovery.inc_degraded_mounts();
    }
    #[inline]
    pub fn inc_recovery_slot_magic_errors(&self) {
        self.recovery.inc_slot_magic_errors();
    }
    #[inline]
    pub fn inc_recovery_epochs_replayed(&self) {
        self.recovery.inc_epochs_replayed();
    }

    // ── Increments pins ───────────────────────────────────────────────────
    #[inline]
    pub fn inc_pins_acquired(&self) {
        self.pins_acquired.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_pins_failed(&self) {
        self.pins_failed.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn update_pin_max(&self, cur: u64) {
        let _ = self
            .pin_max_concurrent
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
                if cur > old {
                    Some(cur)
                } else {
                    None
                }
            });
    }

    // ── Increments snapshots ──────────────────────────────────────────────
    #[inline]
    pub fn inc_snapshots_created(&self) {
        self.snapshots_created.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_snapshots_deleted(&self) {
        self.snapshots_deleted.fetch_add(1, Ordering::Relaxed);
    }

    // ── Increments writeback ──────────────────────────────────────────────
    #[inline]
    pub fn inc_writeback_timer_flush(&self) {
        self.writeback_timer_flushes.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_writeback_pressure_flush(&self) {
        self.writeback_pressure_flushes
            .fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_writeback_full_flush(&self) {
        self.writeback_full_flushes.fetch_add(1, Ordering::Relaxed);
    }

    // ── Increments GC ─────────────────────────────────────────────────────
    #[inline]
    pub fn inc_gc_cycles(&self) {
        self.gc_cycles.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn add_gc_epochs_collected(&self, n: u64) {
        self.gc_epochs_collected.fetch_add(n, Ordering::Relaxed);
    }
    #[inline]
    pub fn add_gc_objects_freed(&self, n: u64) {
        self.gc_objects_freed.fetch_add(n, Ordering::Relaxed);
    }

    // ── Latence ───────────────────────────────────────────────────────────
    #[inline]
    pub fn record_commit_cycles(&self, cycles: u64) {
        self.commit_latency.record(cycles);
    }

    // ── Snapshot instantané ────────────────────────────────────────────────
    /// Prend un snapshot non-bloquant de tous les compteurs.
    pub fn snapshot(&self) -> EpochStatsSnapshot {
        EpochStatsSnapshot {
            commits_ok: self.commits_ok.load(Ordering::Relaxed),
            commits_aborted: self.commits_aborted.load(Ordering::Relaxed),
            forced_commits: self.forced_commits.load(Ordering::Relaxed),
            partial_commits: self.partial_commits.load(Ordering::Relaxed),
            barriers_data: self.barriers_data.load(Ordering::Relaxed),
            barriers_root: self.barriers_root.load(Ordering::Relaxed),
            barriers_record: self.barriers_record.load(Ordering::Relaxed),
            barrier_failures: self.barrier_failures.load(Ordering::Relaxed),
            objects_committed: self.objects_committed.load(Ordering::Relaxed),
            objects_deleted: self.objects_deleted.load(Ordering::Relaxed),
            objects_created: self.objects_created.load(Ordering::Relaxed),
            chained_root_pages: self.chained_root_pages.load(Ordering::Relaxed),
            roots_with_deletions: self.roots_with_deletions.load(Ordering::Relaxed),
            root_page_errors: self.root_page_errors.load(Ordering::Relaxed),
            recovery_slot_io_errors: self.recovery.slot_io_errors.load(Ordering::Relaxed),
            recovery_checksum_errors: self.recovery.checksum_errors.load(Ordering::Relaxed),
            degraded_mounts: self.recovery.degraded_mounts.load(Ordering::Relaxed),
            slot_magic_errors: self.recovery.slot_magic_errors.load(Ordering::Relaxed),
            gc_epochs_collected: self.gc_epochs_collected.load(Ordering::Relaxed),
            gc_objects_freed: self.gc_objects_freed.load(Ordering::Relaxed),
            gc_cycles: self.gc_cycles.load(Ordering::Relaxed),
            snapshots_created: self.snapshots_created.load(Ordering::Relaxed),
            pins_acquired: self.pins_acquired.load(Ordering::Relaxed),
            commit_latency: self.commit_latency.snapshot(),
            recovery: EpochRecoveryStatsSnapshot {
                slot_io_errors: self.recovery.slot_io_errors.load(Ordering::Relaxed),
                checksum_errors: self.recovery.checksum_errors.load(Ordering::Relaxed),
                degraded_mounts: self.recovery.degraded_mounts.load(Ordering::Relaxed),
                slot_magic_errors: self.recovery.slot_magic_errors.load(Ordering::Relaxed),
                epochs_replayed: self.recovery.epochs_replayed.load(Ordering::Relaxed),
            },
        }
    }

    /// Score de santé du sous-système epoch (0 = mort, 100 = parfait).
    pub fn health_score(&self) -> u8 {
        let snp = self.snapshot();
        let total = snp.commits_ok.saturating_add(snp.commits_aborted);
        if total == 0 {
            return 100; // Pas encore de commits.
        }
        let abort_pct = snp.commits_aborted.saturating_mul(100) / total;
        let partial = snp.partial_commits;
        if partial > 0 {
            return 0; // Commits partiaux = critique.
        }
        if abort_pct > 50 {
            return 20;
        }
        if abort_pct > 20 {
            return 50;
        }
        if abort_pct > 5 {
            return 75;
        }
        if snp.barrier_failures > 0 {
            return 80;
        }
        100
    }
}

/// Snapshot des compteurs epoch (non-atomique, pour affichage).
#[derive(Clone, Debug)]
pub struct EpochStatsSnapshot {
    pub commits_ok: u64,
    pub commits_aborted: u64,
    pub forced_commits: u64,
    pub partial_commits: u64,
    pub barriers_data: u64,
    pub barriers_root: u64,
    pub barriers_record: u64,
    pub barrier_failures: u64,
    pub objects_committed: u64,
    pub objects_deleted: u64,
    pub objects_created: u64,
    pub chained_root_pages: u64,
    pub roots_with_deletions: u64,
    pub root_page_errors: u64,
    pub recovery_slot_io_errors: u64,
    pub recovery_checksum_errors: u64,
    pub degraded_mounts: u64,
    pub slot_magic_errors: u64,
    pub gc_epochs_collected: u64,
    pub gc_objects_freed: u64,
    pub gc_cycles: u64,
    pub snapshots_created: u64,
    pub pins_acquired: u64,
    pub commit_latency: LatencyHistogramSnapshot,
    /// Sous-snapshot de recovery (pour accès via `.recovery.xxx`).
    pub recovery: EpochRecoveryStatsSnapshot,
}

impl fmt::Display for EpochStatsSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "EpochStats{{ ok={} abort={} partial={} bars=[{}/{}/{}] \
             objs=[+{} ~{} -{}/gc:{}] latency=[avg={} p99={}] \
             recovery=[io_err={} cksum={} magic={}] }}",
            self.commits_ok,
            self.commits_aborted,
            self.partial_commits,
            self.barriers_data,
            self.barriers_root,
            self.barriers_record,
            self.objects_created,
            self.objects_committed,
            self.objects_deleted,
            self.gc_objects_freed,
            self.commit_latency.avg_cycles,
            self.commit_latency.p99_cycles(),
            self.recovery_slot_io_errors,
            self.recovery_checksum_errors,
            self.slot_magic_errors,
        )
    }
}

// =============================================================================
// Singleton global
// =============================================================================

/// Singleton global des statistiques de l'epoch manager.
///
/// Utilisation : `EPOCH_STATS.inc_commits_ok();`
pub static EPOCH_STATS: EpochStats = EpochStats::new();
