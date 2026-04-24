// kernel/src/fs/exofs/core/stats.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Stats globales ExoFS — compteurs AtomicU64 (JAMAIS dans structs on-disk)
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE ONDISK-03 : AtomicU64 INTERDIT dans les structs on-disk.
// Ces compteurs sont en RAM uniquement, réinitialisés au montage.

use core::sync::atomic::{AtomicU64, Ordering};

/// Compteurs globaux de performance et de santé ExoFS.
///
/// Lus par l'interface d'observabilité et le health check.
pub struct ExofsStats {
    // ── Accès objets ─────────────────────────────────────────────────────────
    pub objects_created: AtomicU64,
    pub objects_deleted: AtomicU64,
    pub objects_read: AtomicU64,
    pub objects_written: AtomicU64,

    // ── Blobs ────────────────────────────────────────────────────────────────
    pub blobs_created: AtomicU64,
    pub blobs_deduped: AtomicU64,
    pub blobs_gc_collected: AtomicU64,

    // ── Epochs ───────────────────────────────────────────────────────────────
    pub epochs_committed: AtomicU64,
    pub epoch_commit_ns_total: AtomicU64, // nanosecondes cumulées

    // ── Chemins ──────────────────────────────────────────────────────────────
    pub path_lookups: AtomicU64,
    pub path_cache_hits: AtomicU64,
    pub path_cache_misses: AtomicU64,
    pub path_index_splits: AtomicU64,

    // ── GC ───────────────────────────────────────────────────────────────────
    pub gc_cycles: AtomicU64,
    pub gc_objects_freed: AtomicU64,
    pub gc_bytes_freed: AtomicU64,

    // ── I/O ──────────────────────────────────────────────────────────────────
    pub io_read_bytes: AtomicU64,
    pub io_write_bytes: AtomicU64,
    pub io_errors: AtomicU64,

    // ── Sécurité ─────────────────────────────────────────────────────────────
    pub cap_denials: AtomicU64,
    pub quota_exceeded: AtomicU64,
}

impl ExofsStats {
    /// Crée les compteurs initialisés à zéro.
    pub const fn new() -> Self {
        Self {
            objects_created: AtomicU64::new(0),
            objects_deleted: AtomicU64::new(0),
            objects_read: AtomicU64::new(0),
            objects_written: AtomicU64::new(0),
            blobs_created: AtomicU64::new(0),
            blobs_deduped: AtomicU64::new(0),
            blobs_gc_collected: AtomicU64::new(0),
            epochs_committed: AtomicU64::new(0),
            epoch_commit_ns_total: AtomicU64::new(0),
            path_lookups: AtomicU64::new(0),
            path_cache_hits: AtomicU64::new(0),
            path_cache_misses: AtomicU64::new(0),
            path_index_splits: AtomicU64::new(0),
            gc_cycles: AtomicU64::new(0),
            gc_objects_freed: AtomicU64::new(0),
            gc_bytes_freed: AtomicU64::new(0),
            io_read_bytes: AtomicU64::new(0),
            io_write_bytes: AtomicU64::new(0),
            io_errors: AtomicU64::new(0),
            cap_denials: AtomicU64::new(0),
            quota_exceeded: AtomicU64::new(0),
        }
    }

    #[inline]
    pub fn inc_objects_created(&self) {
        self.objects_created.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_objects_deleted(&self) {
        self.objects_deleted.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_objects_read(&self) {
        self.objects_read.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_objects_written(&self) {
        self.objects_written.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_blobs_created(&self) {
        self.blobs_created.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_blobs_deduped(&self) {
        self.blobs_deduped.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_blobs_gc_collected(&self) {
        self.blobs_gc_collected.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_epochs_committed(&self) {
        self.epochs_committed.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn add_epoch_commit_ns(&self, ns: u64) {
        self.epoch_commit_ns_total.fetch_add(ns, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_path_lookups(&self) {
        self.path_lookups.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_path_cache_hit(&self) {
        self.path_cache_hits.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_path_cache_miss(&self) {
        self.path_cache_misses.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_path_index_splits(&self) {
        self.path_index_splits.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_gc_cycles(&self) {
        self.gc_cycles.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn add_gc_objects_freed(&self, n: u64) {
        self.gc_objects_freed.fetch_add(n, Ordering::Relaxed);
    }
    #[inline]
    pub fn add_gc_bytes_freed(&self, n: u64) {
        self.gc_bytes_freed.fetch_add(n, Ordering::Relaxed);
    }
    #[inline]
    pub fn add_io_read(&self, n: u64) {
        self.io_read_bytes.fetch_add(n, Ordering::Relaxed);
    }
    #[inline]
    pub fn add_io_write(&self, n: u64) {
        self.io_write_bytes.fetch_add(n, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_io_errors(&self) {
        self.io_errors.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_cap_denials(&self) {
        self.cap_denials.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_quota_exceeded(&self) {
        self.quota_exceeded.fetch_add(1, Ordering::Relaxed);
    }
    // ── Récupération / intégrité ─────────────────────────────────────────────
    #[inline]
    pub fn inc_slot_magic_errors(&self) {
        self.io_errors.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_recovery_slot_io_errors(&self) {
        self.io_errors.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_recovery_checksum_errors(&self) {
        self.io_errors.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_recovery_degraded_mounts(&self) {
        self.io_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Snapshot des compteurs pour observabilité (valeurs approximatives).
    pub fn snapshot(&self) -> ExofsStatsSnapshot {
        ExofsStatsSnapshot {
            objects_created: self.objects_created.load(Ordering::Relaxed),
            objects_deleted: self.objects_deleted.load(Ordering::Relaxed),
            objects_read: self.objects_read.load(Ordering::Relaxed),
            objects_written: self.objects_written.load(Ordering::Relaxed),
            blobs_created: self.blobs_created.load(Ordering::Relaxed),
            blobs_deduped: self.blobs_deduped.load(Ordering::Relaxed),
            blobs_gc_collected: self.blobs_gc_collected.load(Ordering::Relaxed),
            epochs_committed: self.epochs_committed.load(Ordering::Relaxed),
            path_lookups: self.path_lookups.load(Ordering::Relaxed),
            path_cache_hits: self.path_cache_hits.load(Ordering::Relaxed),
            gc_cycles: self.gc_cycles.load(Ordering::Relaxed),
            gc_objects_freed: self.gc_objects_freed.load(Ordering::Relaxed),
            io_read_bytes: self.io_read_bytes.load(Ordering::Relaxed),
            io_write_bytes: self.io_write_bytes.load(Ordering::Relaxed),
            io_errors: self.io_errors.load(Ordering::Relaxed),
            cap_denials: self.cap_denials.load(Ordering::Relaxed),
        }
    }
}

/// Snapshot immuable des stats pour lecture lock-free.
#[derive(Debug, Clone, Copy)]
pub struct ExofsStatsSnapshot {
    pub objects_created: u64,
    pub objects_deleted: u64,
    pub objects_read: u64,
    pub objects_written: u64,
    pub blobs_created: u64,
    pub blobs_deduped: u64,
    pub blobs_gc_collected: u64,
    pub epochs_committed: u64,
    pub path_lookups: u64,
    pub path_cache_hits: u64,
    pub gc_cycles: u64,
    pub gc_objects_freed: u64,
    pub io_read_bytes: u64,
    pub io_write_bytes: u64,
    pub io_errors: u64,
    pub cap_denials: u64,
}

/// Instance globale des stats ExoFS — initialisée au montage.
pub static EXOFS_STATS: ExofsStats = ExofsStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// Stats dérivées sur le snapshot
// ─────────────────────────────────────────────────────────────────────────────

impl ExofsStatsSnapshot {
    /// Taux de hit du cache de chemins en pourcent (entier 0-100).
    ///
    /// Retourne 0 si aucune recherche n'a été effectuée.
    pub fn path_cache_hit_rate_pct(&self) -> u64 {
        let total = self.path_lookups;
        if total == 0 {
            return 0;
        }
        (self.path_cache_hits * 100) / total
    }

    /// Durée moyenne d'un commit epoch en nanosecondes.
    ///
    /// Retourne 0 si aucun commit n'a eu lieu.
    pub fn avg_epoch_commit_ns(&self) -> u64 {
        // Note : epoch_commit_ns_total n'est pas dans le snapshot.
        // Utilisez ExofsStats::avg_epoch_commit_ns() directement.
        0
    }

    /// Taux de déduplication en pourcent (blobs économisés / blobs créés × 100).
    pub fn dedup_ratio_pct(&self) -> u64 {
        let total = self.blobs_created;
        if total == 0 {
            return 0;
        }
        (self.blobs_deduped * 100) / total
    }

    /// Total d'objets vivants (créés - supprimés).
    pub fn objects_alive(&self) -> u64 {
        self.objects_created.saturating_sub(self.objects_deleted)
    }

    /// Taille moyenne lue par opération (octets, entier).
    pub fn avg_read_bytes_per_op(&self) -> u64 {
        if self.objects_read == 0 {
            return 0;
        }
        self.io_read_bytes / self.objects_read
    }

    /// Taille moyenne écrite par opération (octets, entier).
    pub fn avg_write_bytes_per_op(&self) -> u64 {
        if self.objects_written == 0 {
            return 0;
        }
        self.io_write_bytes / self.objects_written
    }

    /// Efficacité GC : pourcent d'objets créés collectés.
    pub fn gc_efficiency_pct(&self) -> u64 {
        if self.objects_created == 0 {
            return 0;
        }
        (self.gc_objects_freed * 100) / self.objects_created
    }

    /// Taux d'erreurs I/O pour 1000 ops (read + write).
    pub fn io_error_rate_per_1k(&self) -> u64 {
        let total_ops = self.objects_read + self.objects_written;
        if total_ops == 0 {
            return 0;
        }
        (self.io_errors * 1000) / total_ops
    }
}

impl ExofsStats {
    /// Durée moyenne d'un commit epoch en nanosecondes.
    pub fn avg_epoch_commit_ns(&self) -> u64 {
        let n = self.epochs_committed.load(Ordering::Relaxed);
        if n == 0 {
            return 0;
        }
        self.epoch_commit_ns_total.load(Ordering::Relaxed) / n
    }

    /// Remet tous les compteurs à zéro (pour les tests ou les resets de stats).
    ///
    /// ATTENTION : opération non atomique — les compteurs peuvent être incohérents
    /// pendant la remise à zéro. Ne pas utiliser en production sans barrière.
    pub fn reset(&self) {
        self.objects_created.store(0, Ordering::Relaxed);
        self.objects_deleted.store(0, Ordering::Relaxed);
        self.objects_read.store(0, Ordering::Relaxed);
        self.objects_written.store(0, Ordering::Relaxed);
        self.blobs_created.store(0, Ordering::Relaxed);
        self.blobs_deduped.store(0, Ordering::Relaxed);
        self.blobs_gc_collected.store(0, Ordering::Relaxed);
        self.epochs_committed.store(0, Ordering::Relaxed);
        self.epoch_commit_ns_total.store(0, Ordering::Relaxed);
        self.path_lookups.store(0, Ordering::Relaxed);
        self.path_cache_hits.store(0, Ordering::Relaxed);
        self.path_cache_misses.store(0, Ordering::Relaxed);
        self.path_index_splits.store(0, Ordering::Relaxed);
        self.gc_cycles.store(0, Ordering::Relaxed);
        self.gc_objects_freed.store(0, Ordering::Relaxed);
        self.gc_bytes_freed.store(0, Ordering::Relaxed);
        self.io_read_bytes.store(0, Ordering::Relaxed);
        self.io_write_bytes.store(0, Ordering::Relaxed);
        self.io_errors.store(0, Ordering::Relaxed);
        self.cap_denials.store(0, Ordering::Relaxed);
        self.quota_exceeded.store(0, Ordering::Relaxed);
    }

    /// Snapshot étendu incluant les stats dérivées.
    pub fn snapshot_extended(&self) -> ExofsStatsExtended {
        let s = self.snapshot();
        let avg_commit_ns = self.avg_epoch_commit_ns();
        ExofsStatsExtended {
            base: s,
            avg_commit_ns,
            dedup_ratio_pct: if s.blobs_created == 0 {
                0
            } else {
                (s.blobs_deduped * 100) / s.blobs_created
            },
            path_hit_pct: if s.path_lookups == 0 {
                0
            } else {
                (s.path_cache_hits * 100) / s.path_lookups
            },
            objects_alive: s.objects_created.saturating_sub(s.objects_deleted),
        }
    }
}

/// Snapshot étendu avec stats dérivées précalculées.
#[derive(Debug, Clone, Copy)]
pub struct ExofsStatsExtended {
    pub base: ExofsStatsSnapshot,
    /// Durée moyenne d'un commit en nanosecondes.
    pub avg_commit_ns: u64,
    /// Taux de déduplication en pourcent (blobs_deduped * 100 / blobs_created).
    pub dedup_ratio_pct: u64,
    /// Taux de hit du cache de chemin en pourcent.
    pub path_hit_pct: u64,
    /// Nombre d'objets vivants (créés - supprimés).
    pub objects_alive: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// ExofsStatsPerKind — compteurs d'accès par ObjectKind (6 variants)
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre de variants dans ObjectKind.
const KIND_COUNT: usize = 6;

/// Indices des variants ObjectKind dans les tableaux de stats.
/// Doit rester synchronisé avec `ObjectKind` dans object_kind.rs.
pub mod kind_idx {
    pub const BLOB: usize = 0;
    pub const CODE: usize = 1;
    pub const CONFIG: usize = 2;
    pub const SECRET: usize = 3;
    pub const PATH_INDEX: usize = 4;
    pub const RELATION: usize = 5;
}

/// Compteurs d'accès séparés par `ObjectKind`.
///
/// Le tableau `[AtomicU64; KIND_COUNT]` permet des mises à jour sans contention
/// entre les threads traitant des kinds différents.
pub struct ExofsStatsPerKind {
    /// Lectures par kind (opérations get_blob / read_content).
    pub reads: [AtomicU64; KIND_COUNT],
    /// Écritures par kind (commit_write / cow_commit).
    pub writes: [AtomicU64; KIND_COUNT],
    /// Créations d'objets par kind.
    pub creates: [AtomicU64; KIND_COUNT],
    /// Suppressions d'objets par kind.
    pub deletes: [AtomicU64; KIND_COUNT],
    /// Octets lus par kind (cumul depuis montage).
    pub bytes_read: [AtomicU64; KIND_COUNT],
    /// Octets écrits par kind (cumul depuis montage).
    pub bytes_written: [AtomicU64; KIND_COUNT],
}

impl ExofsStatsPerKind {
    /// Crée des compteurs à zéro.
    pub const fn new() -> Self {
        const Z: AtomicU64 = AtomicU64::new(0);
        Self {
            reads: [Z, Z, Z, Z, Z, Z],
            writes: [Z, Z, Z, Z, Z, Z],
            creates: [Z, Z, Z, Z, Z, Z],
            deletes: [Z, Z, Z, Z, Z, Z],
            bytes_read: [Z, Z, Z, Z, Z, Z],
            bytes_written: [Z, Z, Z, Z, Z, Z],
        }
    }

    /// Incrémente le compteur de lectures pour un kind donné (index 0..5).
    #[inline]
    pub fn inc_read(&self, kind_index: usize) {
        if kind_index < KIND_COUNT {
            self.reads[kind_index].fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Incrémente le compteur d'écritures pour un kind donné.
    #[inline]
    pub fn inc_write(&self, kind_index: usize) {
        if kind_index < KIND_COUNT {
            self.writes[kind_index].fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Incrémente le compteur de créations.
    #[inline]
    pub fn inc_create(&self, kind_index: usize) {
        if kind_index < KIND_COUNT {
            self.creates[kind_index].fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Incrémente le compteur de suppressions.
    #[inline]
    pub fn inc_delete(&self, kind_index: usize) {
        if kind_index < KIND_COUNT {
            self.deletes[kind_index].fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Ajoute des octets lus.
    #[inline]
    pub fn add_bytes_read(&self, kind_index: usize, bytes: u64) {
        if kind_index < KIND_COUNT {
            self.bytes_read[kind_index].fetch_add(bytes, Ordering::Relaxed);
        }
    }

    /// Ajoute des octets écrits.
    #[inline]
    pub fn add_bytes_written(&self, kind_index: usize, bytes: u64) {
        if kind_index < KIND_COUNT {
            self.bytes_written[kind_index].fetch_add(bytes, Ordering::Relaxed);
        }
    }

    /// Retourne le total des lectures.
    pub fn total_reads(&self) -> u64 {
        self.reads.iter().map(|a| a.load(Ordering::Relaxed)).sum()
    }

    /// Retourne le total des écritures.
    pub fn total_writes(&self) -> u64 {
        self.writes.iter().map(|a| a.load(Ordering::Relaxed)).sum()
    }

    /// Retourne le total des octets lus.
    pub fn total_bytes_read(&self) -> u64 {
        self.bytes_read
            .iter()
            .map(|a| a.load(Ordering::Relaxed))
            .sum()
    }

    /// Remet tous les compteurs à zéro.
    pub fn reset(&self) {
        for i in 0..KIND_COUNT {
            self.reads[i].store(0, Ordering::Relaxed);
            self.writes[i].store(0, Ordering::Relaxed);
            self.creates[i].store(0, Ordering::Relaxed);
            self.deletes[i].store(0, Ordering::Relaxed);
            self.bytes_read[i].store(0, Ordering::Relaxed);
            self.bytes_written[i].store(0, Ordering::Relaxed);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ExofsStatsTier — activation I/O par tier de stockage
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre de tiers de stockage (HOT=0, WARM=1, COLD=2).
const TIER_COUNT: usize = 3;

/// Statistiques d'accès par tier de stockage.
pub struct ExofsStatsTier {
    /// Octets lus par tier.
    pub bytes_read: [AtomicU64; TIER_COUNT],
    /// Octets écrits par tier.
    pub bytes_written: [AtomicU64; TIER_COUNT],
    /// Nombre d'opérations de migration vers ce tier.
    pub migrations_in: [AtomicU64; TIER_COUNT],
    /// Nombre d'opérations de migration hors de ce tier.
    pub migrations_out: [AtomicU64; TIER_COUNT],
}

impl ExofsStatsTier {
    pub const fn new() -> Self {
        const Z: AtomicU64 = AtomicU64::new(0);
        Self {
            bytes_read: [Z, Z, Z],
            bytes_written: [Z, Z, Z],
            migrations_in: [Z, Z, Z],
            migrations_out: [Z, Z, Z],
        }
    }

    #[inline]
    pub fn add_read(&self, tier: usize, bytes: u64) {
        if tier < TIER_COUNT {
            self.bytes_read[tier].fetch_add(bytes, Ordering::Relaxed);
        }
    }

    #[inline]
    pub fn add_write(&self, tier: usize, bytes: u64) {
        if tier < TIER_COUNT {
            self.bytes_written[tier].fetch_add(bytes, Ordering::Relaxed);
        }
    }

    #[inline]
    pub fn inc_migration_in(&self, tier: usize) {
        if tier < TIER_COUNT {
            self.migrations_in[tier].fetch_add(1, Ordering::Relaxed);
        }
    }

    #[inline]
    pub fn inc_migration_out(&self, tier: usize) {
        if tier < TIER_COUNT {
            self.migrations_out[tier].fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Octets totaux lus sur tous les tiers.
    pub fn total_bytes_read(&self) -> u64 {
        self.bytes_read
            .iter()
            .map(|a| a.load(Ordering::Relaxed))
            .sum()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GcPressureGauge — jauge de pression GC
// ─────────────────────────────────────────────────────────────────────────────

/// Jauge de pression du GC, combinant blobs et epochs en attente.
///
/// Le niveau de pression est utilisé par le scheduler GC pour décider
/// si une passe forcée est nécessaire.
pub struct GcPressureGauge {
    /// Nombre de blobs en attente de collection.
    pub pending_blobs: AtomicU64,
    /// Nombre d'epochs en attente de collection.
    pub pending_epochs: AtomicU64,
    /// Nombre d'octets associés aux blobs pendants.
    pub pending_bytes: AtomicU64,
}

impl GcPressureGauge {
    pub const fn new() -> Self {
        Self {
            pending_blobs: AtomicU64::new(0),
            pending_epochs: AtomicU64::new(0),
            pending_bytes: AtomicU64::new(0),
        }
    }

    #[inline]
    pub fn add_blob(&self, blob_bytes: u64) {
        self.pending_blobs.fetch_add(1, Ordering::Relaxed);
        self.pending_bytes.fetch_add(blob_bytes, Ordering::Relaxed);
    }

    #[inline]
    pub fn remove_blob(&self, blob_bytes: u64) {
        let _ = self
            .pending_blobs
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_sub(1))
            });
        let _ = self
            .pending_bytes
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_sub(blob_bytes))
            });
    }

    #[inline]
    pub fn add_epoch(&self) {
        self.pending_epochs.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn remove_epoch(&self) {
        let _ = self
            .pending_epochs
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_sub(1))
            });
    }

    /// Retourne le niveau de pression GC sur 4 niveaux (0=low, 1=medium, 2=high, 3=critical).
    ///
    /// Le calcul est basé sur le nombre de blobs pendants :
    /// - 0..25 blobs  → low
    /// - 25..100      → medium
    /// - 100..500     → high
    /// - ≥500         → critical
    pub fn pressure_level(&self) -> u8 {
        let blobs = self.pending_blobs.load(Ordering::Relaxed);
        if blobs < 25 {
            0
        } else if blobs < 100 {
            1
        } else if blobs < 500 {
            2
        } else {
            3
        }
    }

    /// Retourne `true` si le GC doit être déclenché immédiatement.
    #[inline]
    pub fn requires_immediate_gc(&self) -> bool {
        self.pressure_level() >= 3
    }
}

/// Statistiques globales par kind — instance statique.
pub static EXOFS_STATS_PER_KIND: ExofsStatsPerKind = ExofsStatsPerKind::new();

/// Statistiques par tier — instance statique.
pub static EXOFS_STATS_TIER: ExofsStatsTier = ExofsStatsTier::new();

/// Jauge GC globale — instance statique.
pub static GC_PRESSURE: GcPressureGauge = GcPressureGauge::new();
