// kernel/src/fs/exofs/storage/storage_stats.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Statistiques de stockage ExoFS — agrégation des métriques I/O
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Ce module expose un singleton global `STORAGE_STATS` qui collecte en temps
// réel toutes les métriques du sous-système de stockage.
//
// Règles respectées :
// - ARITH-02 : saturating_add pour les compteurs (jamais d'overflow silencieux).
// - LOCK-04  : pas de SpinLock sur le chemin critique, uniquement AtomicU64.
// - ONDISK-03 : ce module est purement RAM — aucun type on-disk ici.

use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// StorageStats — agrégat de compteurs atomiques
// ─────────────────────────────────────────────────────────────────────────────

/// Statistiques globales du sous-système de stockage ExoFS.
///
/// Tous les compteurs utilisent `AtomicU64` avec `Ordering::Relaxed` pour les
/// incréments (pas de synchronisation requise entre CPUs — les valeurs sont
/// approximatives par construction).
pub struct StorageStats {
    // ── I/O physique ──────────────────────────────────────────────────────
    /// Octets physiquement écrits sur le périphérique bloc.
    pub bytes_written:          AtomicU64,
    /// Octets physiquement lus depuis le périphérique bloc.
    pub bytes_read:             AtomicU64,
    /// Nombre d'opérations d'écriture physique.
    pub write_ops:              AtomicU64,
    /// Nombre d'opérations de lecture physique.
    pub read_ops:               AtomicU64,
    /// Nombre d'erreurs I/O (lecture + écriture).
    pub io_errors:              AtomicU64,
    /// Nombre de ré-essais I/O.
    pub io_retries:             AtomicU64,

    // ── Blobs ─────────────────────────────────────────────────────────────
    /// Blobs créés depuis le montage.
    pub blobs_created:          AtomicU64,
    /// Blobs supprimés (ref_count tombé à 0).
    pub blobs_deleted:          AtomicU64,
    /// Hits de déduplication (blob existant réutilisé).
    pub dedup_hits:             AtomicU64,
    /// Misses de déduplication (blob nouveau écrit).
    pub dedup_misses:           AtomicU64,
    /// Octets économisés par déduplication.
    pub dedup_bytes_saved:      AtomicU64,

    // ── Objets ────────────────────────────────────────────────────────────
    /// Objets écrits.
    pub objects_written:        AtomicU64,
    /// Objets lus.
    pub objects_read:           AtomicU64,
    /// Objets supprimés.
    pub objects_deleted:        AtomicU64,
    /// Erreurs de checksum d'objet.
    pub object_checksum_errors: AtomicU64,

    // ── Compression ───────────────────────────────────────────────────────
    /// Octets avant compression (données brutes).
    pub compress_bytes_in:      AtomicU64,
    /// Octets après compression (données compressées).
    pub compress_bytes_out:     AtomicU64,
    /// Nombre de compressions effectuées.
    pub compress_ops:           AtomicU64,
    /// Nombre de décompressions effectuées.
    pub decompress_ops:         AtomicU64,
    /// Échecs de compression.
    pub compress_errors:        AtomicU64,

    // ── Heap et allocation ────────────────────────────────────────────────
    /// Allocations heap réussies.
    pub heap_allocs:            AtomicU64,
    /// Libérations heap.
    pub heap_frees:             AtomicU64,
    /// Échecs d'allocation (espace insuffisant).
    pub heap_alloc_failures:    AtomicU64,
    /// Octets alloués dans le heap (approximatif).
    pub heap_bytes_used:        AtomicU64,
    /// Passes de coalescence exécutées.
    pub heap_coalesce_runs:     AtomicU64,

    // ── Block cache ───────────────────────────────────────────────────────
    /// Hits block cache (lecture servie depuis RAM).
    pub cache_hits:             AtomicU64,
    /// Misses block cache (lecture physique nécessaire).
    pub cache_misses:           AtomicU64,
    /// Evictions de blocs du cache.
    pub cache_evictions:        AtomicU64,
    /// Blocs dirty flushés vers le disque.
    pub cache_flushes:          AtomicU64,

    // ── Superblock ────────────────────────────────────────────────────────
    /// Nombre de commits superblock réussis.
    pub sb_commits:             AtomicU64,
    /// Nombre d'échecs de commit superblock.
    pub sb_commit_errors:       AtomicU64,
    /// Restaurations depuis un miroir.
    pub sb_mirror_restores:     AtomicU64,

    // ── Checksum ─────────────────────────────────────────────────────────
    /// Vérifications checksum réussies.
    pub checksum_ok:            AtomicU64,
    /// Vérifications checksum échouées.
    pub checksum_errors:        AtomicU64,

    // ── IO batch ─────────────────────────────────────────────────────────
    /// Batches I/O soumis.
    pub io_batches_submitted:   AtomicU64,
    /// Batches I/O complétés.
    pub io_batches_completed:   AtomicU64,
    /// Batches I/O en erreur.
    pub io_batches_errors:      AtomicU64,
    /// Ops fusionnées par le moteur de batch.
    pub io_ops_merged:          AtomicU64,
}

impl StorageStats {
    /// Construit un agrégat zeroisé.
    pub const fn new() -> Self {
        macro_rules! z { () => { AtomicU64::new(0) }; }
        Self {
            bytes_written:          z!(),
            bytes_read:             z!(),
            write_ops:              z!(),
            read_ops:               z!(),
            io_errors:              z!(),
            io_retries:             z!(),
            blobs_created:          z!(),
            blobs_deleted:          z!(),
            dedup_hits:             z!(),
            dedup_misses:           z!(),
            dedup_bytes_saved:      z!(),
            objects_written:        z!(),
            objects_read:           z!(),
            objects_deleted:        z!(),
            object_checksum_errors: z!(),
            compress_bytes_in:      z!(),
            compress_bytes_out:     z!(),
            compress_ops:           z!(),
            decompress_ops:         z!(),
            compress_errors:        z!(),
            heap_allocs:            z!(),
            heap_frees:             z!(),
            heap_alloc_failures:    z!(),
            heap_bytes_used:        z!(),
            heap_coalesce_runs:     z!(),
            cache_hits:             z!(),
            cache_misses:           z!(),
            cache_evictions:        z!(),
            cache_flushes:          z!(),
            sb_commits:             z!(),
            sb_commit_errors:       z!(),
            sb_mirror_restores:     z!(),
            checksum_ok:            z!(),
            checksum_errors:        z!(),
            io_batches_submitted:   z!(),
            io_batches_completed:   z!(),
            io_batches_errors:      z!(),
            io_ops_merged:          z!(),
        }
    }

    // ── I/O ──────────────────────────────────────────────────────────────

    #[inline]
    pub fn add_write(&self, bytes: u64) {
        self.bytes_written.fetch_add(bytes, Ordering::Relaxed);
        self.write_ops.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn add_read(&self, bytes: u64) {
        self.bytes_read.fetch_add(bytes, Ordering::Relaxed);
        self.read_ops.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_io_error(&self) {
        self.io_errors.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_io_retry(&self) {
        self.io_retries.fetch_add(1, Ordering::Relaxed);
    }

    // ── Blobs ─────────────────────────────────────────────────────────────

    #[inline]
    pub fn inc_blob_created(&self) {
        self.blobs_created.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_blob_deleted(&self) {
        self.blobs_deleted.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_dedup_hit(&self, bytes_saved: u64) {
        self.dedup_hits.fetch_add(1, Ordering::Relaxed);
        self.dedup_bytes_saved.fetch_add(bytes_saved, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_dedup_miss(&self) {
        self.dedup_misses.fetch_add(1, Ordering::Relaxed);
    }

    // ── Objets ────────────────────────────────────────────────────────────

    #[inline]
    pub fn inc_object_written(&self) {
        self.objects_written.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_object_read(&self) {
        self.objects_read.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_object_deleted(&self) {
        self.objects_deleted.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_object_checksum_error(&self) {
        self.object_checksum_errors.fetch_add(1, Ordering::Relaxed);
    }

    // ── Compression ───────────────────────────────────────────────────────

    #[inline]
    pub fn add_compression(&self, bytes_in: u64, bytes_out: u64) {
        self.compress_bytes_in.fetch_add(bytes_in, Ordering::Relaxed);
        self.compress_bytes_out.fetch_add(bytes_out, Ordering::Relaxed);
        self.compress_ops.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_decompress(&self) {
        self.decompress_ops.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_compress_error(&self) {
        self.compress_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Ratio de compression en millièmes (1000 = aucune compression).
    /// > 1000 → données plus grandes après compression (expansion).
    pub fn compression_ratio_milli(&self) -> u64 {
        let bytes_in  = self.compress_bytes_in.load(Ordering::Relaxed);
        let bytes_out = self.compress_bytes_out.load(Ordering::Relaxed);
        if bytes_in == 0 { return 1000; }
        (bytes_out as u128 * 1000 / bytes_in as u128) as u64
    }

    // ── Heap ──────────────────────────────────────────────────────────────

    #[inline]
    pub fn inc_heap_alloc(&self, bytes: u64) {
        self.heap_allocs.fetch_add(1, Ordering::Relaxed);
        self.heap_bytes_used.fetch_add(bytes, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_heap_free(&self, bytes: u64) {
        self.heap_frees.fetch_add(1, Ordering::Relaxed);
        self.heap_bytes_used.fetch_sub(
            self.heap_bytes_used.load(Ordering::Relaxed).min(bytes),
            Ordering::Relaxed,
        );
    }

    #[inline]
    pub fn inc_heap_alloc_failure(&self) {
        self.heap_alloc_failures.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_heap_coalesce(&self) {
        self.heap_coalesce_runs.fetch_add(1, Ordering::Relaxed);
    }

    // ── Block cache ───────────────────────────────────────────────────────

    #[inline]
    pub fn inc_cache_hit(&self) {
        self.cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_cache_miss(&self) {
        self.cache_misses.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_cache_eviction(&self) {
        self.cache_evictions.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_cache_flush(&self) {
        self.cache_flushes.fetch_add(1, Ordering::Relaxed);
    }

    /// Ratio de hit cache en pourcentage (0..=100).
    pub fn cache_hit_rate_pct(&self) -> u64 {
        let hits   = self.cache_hits.load(Ordering::Relaxed);
        let misses = self.cache_misses.load(Ordering::Relaxed);
        let total  = hits.saturating_add(misses);
        if total == 0 { return 0; }
        (hits as u128 * 100 / total as u128) as u64
    }

    // ── Superblock ────────────────────────────────────────────────────────

    #[inline]
    pub fn inc_sb_commit(&self) {
        self.sb_commits.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_sb_commit_error(&self) {
        self.sb_commit_errors.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_sb_mirror_restore(&self) {
        self.sb_mirror_restores.fetch_add(1, Ordering::Relaxed);
    }

    // ── Checksum ─────────────────────────────────────────────────────────

    #[inline]
    pub fn inc_checksum_ok(&self) {
        self.checksum_ok.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_checksum_error(&self) {
        self.checksum_errors.fetch_add(1, Ordering::Relaxed);
    }

    // ── IO batch ─────────────────────────────────────────────────────────

    #[inline]
    pub fn inc_batch_submitted(&self) {
        self.io_batches_submitted.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_batch_completed(&self) {
        self.io_batches_completed.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_batch_error(&self) {
        self.io_batches_errors.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn add_ops_merged(&self, n: u64) {
        self.io_ops_merged.fetch_add(n, Ordering::Relaxed);
    }

    // ── Snapshot ─────────────────────────────────────────────────────────

    /// Prend un snapshot cohérent (valeurs approximatives — pas de verrou global).
    pub fn snapshot(&self) -> StorageStatsSnapshot {
        StorageStatsSnapshot {
            bytes_written:          self.bytes_written.load(Ordering::Relaxed),
            bytes_read:             self.bytes_read.load(Ordering::Relaxed),
            write_ops:              self.write_ops.load(Ordering::Relaxed),
            read_ops:               self.read_ops.load(Ordering::Relaxed),
            io_errors:              self.io_errors.load(Ordering::Relaxed),
            io_retries:             self.io_retries.load(Ordering::Relaxed),
            blobs_created:          self.blobs_created.load(Ordering::Relaxed),
            blobs_deleted:          self.blobs_deleted.load(Ordering::Relaxed),
            dedup_hits:             self.dedup_hits.load(Ordering::Relaxed),
            dedup_misses:           self.dedup_misses.load(Ordering::Relaxed),
            dedup_bytes_saved:      self.dedup_bytes_saved.load(Ordering::Relaxed),
            objects_written:        self.objects_written.load(Ordering::Relaxed),
            objects_read:           self.objects_read.load(Ordering::Relaxed),
            objects_deleted:        self.objects_deleted.load(Ordering::Relaxed),
            object_checksum_errors: self.object_checksum_errors.load(Ordering::Relaxed),
            compress_bytes_in:      self.compress_bytes_in.load(Ordering::Relaxed),
            compress_bytes_out:     self.compress_bytes_out.load(Ordering::Relaxed),
            compress_ops:           self.compress_ops.load(Ordering::Relaxed),
            decompress_ops:         self.decompress_ops.load(Ordering::Relaxed),
            compress_errors:        self.compress_errors.load(Ordering::Relaxed),
            heap_allocs:            self.heap_allocs.load(Ordering::Relaxed),
            heap_frees:             self.heap_frees.load(Ordering::Relaxed),
            heap_alloc_failures:    self.heap_alloc_failures.load(Ordering::Relaxed),
            heap_bytes_used:        self.heap_bytes_used.load(Ordering::Relaxed),
            heap_coalesce_runs:     self.heap_coalesce_runs.load(Ordering::Relaxed),
            cache_hits:             self.cache_hits.load(Ordering::Relaxed),
            cache_misses:           self.cache_misses.load(Ordering::Relaxed),
            cache_evictions:        self.cache_evictions.load(Ordering::Relaxed),
            cache_flushes:          self.cache_flushes.load(Ordering::Relaxed),
            sb_commits:             self.sb_commits.load(Ordering::Relaxed),
            sb_commit_errors:       self.sb_commit_errors.load(Ordering::Relaxed),
            sb_mirror_restores:     self.sb_mirror_restores.load(Ordering::Relaxed),
            checksum_ok:            self.checksum_ok.load(Ordering::Relaxed),
            checksum_errors:        self.checksum_errors.load(Ordering::Relaxed),
            io_batches_submitted:   self.io_batches_submitted.load(Ordering::Relaxed),
            io_batches_completed:   self.io_batches_completed.load(Ordering::Relaxed),
            io_batches_errors:      self.io_batches_errors.load(Ordering::Relaxed),
            io_ops_merged:          self.io_ops_merged.load(Ordering::Relaxed),
        }
    }

    /// Remet tous les compteurs à zéro (usage : tests, maintenance).
    pub fn reset(&self) {
        macro_rules! zero { ($field:expr) => { $field.store(0, Ordering::Relaxed); }; }
        zero!(self.bytes_written);
        zero!(self.bytes_read);
        zero!(self.write_ops);
        zero!(self.read_ops);
        zero!(self.io_errors);
        zero!(self.io_retries);
        zero!(self.blobs_created);
        zero!(self.blobs_deleted);
        zero!(self.dedup_hits);
        zero!(self.dedup_misses);
        zero!(self.dedup_bytes_saved);
        zero!(self.objects_written);
        zero!(self.objects_read);
        zero!(self.objects_deleted);
        zero!(self.object_checksum_errors);
        zero!(self.compress_bytes_in);
        zero!(self.compress_bytes_out);
        zero!(self.compress_ops);
        zero!(self.decompress_ops);
        zero!(self.compress_errors);
        zero!(self.heap_allocs);
        zero!(self.heap_frees);
        zero!(self.heap_alloc_failures);
        zero!(self.heap_bytes_used);
        zero!(self.heap_coalesce_runs);
        zero!(self.cache_hits);
        zero!(self.cache_misses);
        zero!(self.cache_evictions);
        zero!(self.cache_flushes);
        zero!(self.sb_commits);
        zero!(self.sb_commit_errors);
        zero!(self.sb_mirror_restores);
        zero!(self.checksum_ok);
        zero!(self.checksum_errors);
        zero!(self.io_batches_submitted);
        zero!(self.io_batches_completed);
        zero!(self.io_batches_errors);
        zero!(self.io_ops_merged);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// StorageStatsSnapshot — instantané pour reporting
// ─────────────────────────────────────────────────────────────────────────────

/// Valeurs instantanées de tous les compteurs (copiées depuis StorageStats).
#[derive(Clone, Debug, Default)]
pub struct StorageStatsSnapshot {
    pub bytes_written:          u64,
    pub bytes_read:             u64,
    pub write_ops:              u64,
    pub read_ops:               u64,
    pub io_errors:              u64,
    pub io_retries:             u64,
    pub blobs_created:          u64,
    pub blobs_deleted:          u64,
    pub dedup_hits:             u64,
    pub dedup_misses:           u64,
    pub dedup_bytes_saved:      u64,
    pub objects_written:        u64,
    pub objects_read:           u64,
    pub objects_deleted:        u64,
    pub object_checksum_errors: u64,
    pub compress_bytes_in:      u64,
    pub compress_bytes_out:     u64,
    pub compress_ops:           u64,
    pub decompress_ops:         u64,
    pub compress_errors:        u64,
    pub heap_allocs:            u64,
    pub heap_frees:             u64,
    pub heap_alloc_failures:    u64,
    pub heap_bytes_used:        u64,
    pub heap_coalesce_runs:     u64,
    pub cache_hits:             u64,
    pub cache_misses:           u64,
    pub cache_evictions:        u64,
    pub cache_flushes:          u64,
    pub sb_commits:             u64,
    pub sb_commit_errors:       u64,
    pub sb_mirror_restores:     u64,
    pub checksum_ok:            u64,
    pub checksum_errors:        u64,
    pub io_batches_submitted:   u64,
    pub io_batches_completed:   u64,
    pub io_batches_errors:      u64,
    pub io_ops_merged:          u64,
}

impl StorageStatsSnapshot {
    /// Ratio de déduplication (0..=100).
    pub fn dedup_rate_pct(&self) -> u64 {
        let total = self.dedup_hits.saturating_add(self.dedup_misses);
        if total == 0 { return 0; }
        (self.dedup_hits as u128 * 100 / total as u128) as u64
    }

    /// Ratio cache hits (0..=100).
    pub fn cache_hit_rate_pct(&self) -> u64 {
        let total = self.cache_hits.saturating_add(self.cache_misses);
        if total == 0 { return 0; }
        (self.cache_hits as u128 * 100 / total as u128) as u64
    }

    /// Ratio de compression en millièmes (1000 = aucun gain).
    pub fn compression_ratio_milli(&self) -> u64 {
        if self.compress_bytes_in == 0 { return 1000; }
        (self.compress_bytes_out as u128 * 1000 / self.compress_bytes_in as u128) as u64
    }

    /// Taux d'erreur I/O en millièmes (0 = parfait).
    pub fn io_error_rate_milli(&self) -> u64 {
        let total = self.write_ops.saturating_add(self.read_ops);
        if total == 0 { return 0; }
        (self.io_errors as u128 * 1000 / total as u128) as u64
    }

    /// Delta entre deux snapshots (utile pour les intervalles de reporting).
    pub fn delta(&self, prev: &StorageStatsSnapshot) -> StorageStatsSnapshot {
        macro_rules! diff { ($f:ident) => { self.$f.saturating_sub(prev.$f) }; }
        StorageStatsSnapshot {
            bytes_written:          diff!(bytes_written),
            bytes_read:             diff!(bytes_read),
            write_ops:              diff!(write_ops),
            read_ops:               diff!(read_ops),
            io_errors:              diff!(io_errors),
            io_retries:             diff!(io_retries),
            blobs_created:          diff!(blobs_created),
            blobs_deleted:          diff!(blobs_deleted),
            dedup_hits:             diff!(dedup_hits),
            dedup_misses:           diff!(dedup_misses),
            dedup_bytes_saved:      diff!(dedup_bytes_saved),
            objects_written:        diff!(objects_written),
            objects_read:           diff!(objects_read),
            objects_deleted:        diff!(objects_deleted),
            object_checksum_errors: diff!(object_checksum_errors),
            compress_bytes_in:      diff!(compress_bytes_in),
            compress_bytes_out:     diff!(compress_bytes_out),
            compress_ops:           diff!(compress_ops),
            decompress_ops:         diff!(decompress_ops),
            compress_errors:        diff!(compress_errors),
            heap_allocs:            diff!(heap_allocs),
            heap_frees:             diff!(heap_frees),
            heap_alloc_failures:    diff!(heap_alloc_failures),
            heap_bytes_used:        diff!(heap_bytes_used),
            heap_coalesce_runs:     diff!(heap_coalesce_runs),
            cache_hits:             diff!(cache_hits),
            cache_misses:           diff!(cache_misses),
            cache_evictions:        diff!(cache_evictions),
            cache_flushes:          diff!(cache_flushes),
            sb_commits:             diff!(sb_commits),
            sb_commit_errors:       diff!(sb_commit_errors),
            sb_mirror_restores:     diff!(sb_mirror_restores),
            checksum_ok:            diff!(checksum_ok),
            checksum_errors:        diff!(checksum_errors),
            io_batches_submitted:   diff!(io_batches_submitted),
            io_batches_completed:   diff!(io_batches_completed),
            io_batches_errors:      diff!(io_batches_errors),
            io_ops_merged:          diff!(io_ops_merged),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Singleton global
// ─────────────────────────────────────────────────────────────────────────────

/// Singleton de statistiques de stockage.
pub static STORAGE_STATS: StorageStats = StorageStats::new();
