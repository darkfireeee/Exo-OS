// kernel/src/fs/exofs/core/config.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Config — paramètres boot-time ExoFS (tailles caches, seuils GC, profils)
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Cette structure est initialisée au montage depuis les options de montage
// et les constantes par défaut. Elle est ensuite immuable.

use crate::fs::exofs::core::error::ExofsError;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Configuration runtime ExoFS — fixée au montage, immuable après.
pub struct ExofsConfig {
    // ── Caches ───────────────────────────────────────────────────────────────
    /// Taille du cache d'objets logiques (en nombre d'entrées).
    pub object_cache_size: AtomicUsize,
    /// Taille du cache de blobs physiques (en nombre d'entrées).
    pub blob_cache_size: AtomicUsize,
    /// Taille du cache de chemins dentry (en nombre d'entrées).
    pub path_cache_size: AtomicUsize,
    /// Taille du cache d'extents (en nombre d'entrées).
    pub extent_cache_size: AtomicUsize,

    // ── GC ───────────────────────────────────────────────────────────────────
    /// Seuil espace libre GC en % (déclenche GC si free < threshold).
    pub gc_free_threshold_pct: AtomicU64,
    /// Intervalle du timer GC en secondes.
    pub gc_timer_secs: AtomicU64,
    /// Délai minimum entre déduplication et GC en epochs.
    pub gc_min_epoch_delay: AtomicU64,
    /// Nombre maximum d'objets GC par cycle (évite les latences).
    pub gc_max_objects_per_cycle: AtomicU64,

    // ── Writeback ────────────────────────────────────────────────────────────
    /// Intervalle writeback en millisecondes.
    pub writeback_interval_ms: AtomicU64,
    /// Nombre maximum d'objets dirty avant writeback forcé.
    pub writeback_dirty_max: AtomicUsize,

    // ── Compression ──────────────────────────────────────────────────────────
    /// Taille minimale pour activer la compression (en octets).
    pub compress_min_size: AtomicUsize,
    /// Niveau de compression (1 = rapide, 9 = meilleur ratio).
    pub compress_level: AtomicUsize,

    // ── Déduplication ────────────────────────────────────────────────────────
    /// Taille minimale pour la déduplication (en octets).
    pub dedup_min_size: AtomicUsize,

    // ── I/O ──────────────────────────────────────────────────────────────────
    /// Profondeur de la file de requêtes I/O.
    pub io_queue_depth: AtomicUsize,
    /// Taille du buffer de lecture préventive (octets, 0 = désactivé).
    pub readahead_size: AtomicU64,

    // ── Sécurité ─────────────────────────────────────────────────────────────
    /// Vrai si les checksums sont vérifiés systématiquement en lecture.
    pub verify_checksums_on_read: AtomicUsize, // 0 = false, 1 = true
    /// Vrai si le chiffrement est requis pour les blobs Secret.
    pub require_encryption_for_secrets: AtomicUsize,
}

impl ExofsConfig {
    /// Crée une configuration avec les valeurs par défaut équilibrées.
    pub const fn default_config() -> Self {
        Self {
            object_cache_size: AtomicUsize::new(4096),
            blob_cache_size: AtomicUsize::new(8192),
            path_cache_size: AtomicUsize::new(10_000),
            extent_cache_size: AtomicUsize::new(4096),
            gc_free_threshold_pct: AtomicU64::new(20),
            gc_timer_secs: AtomicU64::new(60),
            gc_min_epoch_delay: AtomicU64::new(2),
            gc_max_objects_per_cycle: AtomicU64::new(1000),
            writeback_interval_ms: AtomicU64::new(1),
            writeback_dirty_max: AtomicUsize::new(500),
            compress_min_size: AtomicUsize::new(512),
            compress_level: AtomicUsize::new(3),
            dedup_min_size: AtomicUsize::new(4096),
            io_queue_depth: AtomicUsize::new(32),
            readahead_size: AtomicU64::new(131_072), // 128 KiB
            verify_checksums_on_read: AtomicUsize::new(1),
            require_encryption_for_secrets: AtomicUsize::new(1),
        }
    }

    // ── Accesseurs ────────────────────────────────────────────────────────────

    #[inline]
    pub fn object_cache_size(&self) -> usize {
        self.object_cache_size.load(Ordering::Relaxed)
    }
    #[inline]
    pub fn blob_cache_size(&self) -> usize {
        self.blob_cache_size.load(Ordering::Relaxed)
    }
    #[inline]
    pub fn path_cache_size(&self) -> usize {
        self.path_cache_size.load(Ordering::Relaxed)
    }
    #[inline]
    pub fn extent_cache_size(&self) -> usize {
        self.extent_cache_size.load(Ordering::Relaxed)
    }
    #[inline]
    pub fn gc_free_threshold_pct(&self) -> u64 {
        self.gc_free_threshold_pct.load(Ordering::Relaxed)
    }
    #[inline]
    pub fn gc_timer_secs(&self) -> u64 {
        self.gc_timer_secs.load(Ordering::Relaxed)
    }
    #[inline]
    pub fn gc_min_epoch_delay(&self) -> u64 {
        self.gc_min_epoch_delay.load(Ordering::Relaxed)
    }
    #[inline]
    pub fn gc_max_objects_per_cycle(&self) -> u64 {
        self.gc_max_objects_per_cycle.load(Ordering::Relaxed)
    }
    #[inline]
    pub fn writeback_interval_ms(&self) -> u64 {
        self.writeback_interval_ms.load(Ordering::Relaxed)
    }
    #[inline]
    pub fn writeback_dirty_max(&self) -> usize {
        self.writeback_dirty_max.load(Ordering::Relaxed)
    }
    #[inline]
    pub fn compress_min_size(&self) -> usize {
        self.compress_min_size.load(Ordering::Relaxed)
    }
    #[inline]
    pub fn compress_level(&self) -> usize {
        self.compress_level.load(Ordering::Relaxed)
    }
    #[inline]
    pub fn dedup_min_size(&self) -> usize {
        self.dedup_min_size.load(Ordering::Relaxed)
    }
    #[inline]
    pub fn io_queue_depth(&self) -> usize {
        self.io_queue_depth.load(Ordering::Relaxed)
    }
    #[inline]
    pub fn readahead_size(&self) -> u64 {
        self.readahead_size.load(Ordering::Relaxed)
    }
    #[inline]
    pub fn verify_checksums(&self) -> bool {
        self.verify_checksums_on_read.load(Ordering::Relaxed) != 0
    }
    #[inline]
    pub fn require_encryption_for_secrets(&self) -> bool {
        self.require_encryption_for_secrets.load(Ordering::Relaxed) != 0
    }

    // ── Setters ────────────────────────────────────────────────────────────────

    pub fn set_object_cache_size(&self, n: usize) {
        self.object_cache_size.store(n, Ordering::Relaxed);
    }
    pub fn set_blob_cache_size(&self, n: usize) {
        self.blob_cache_size.store(n, Ordering::Relaxed);
    }
    pub fn set_path_cache_size(&self, n: usize) {
        self.path_cache_size.store(n, Ordering::Relaxed);
    }
    pub fn set_gc_timer_secs(&self, n: u64) {
        self.gc_timer_secs.store(n, Ordering::Relaxed);
    }
    pub fn set_gc_free_threshold_pct(&self, n: u64) {
        self.gc_free_threshold_pct.store(n, Ordering::Relaxed);
    }
    pub fn set_writeback_interval_ms(&self, n: u64) {
        self.writeback_interval_ms.store(n, Ordering::Relaxed);
    }
    pub fn set_compress_level(&self, n: usize) {
        self.compress_level.store(n, Ordering::Relaxed);
    }
    pub fn set_readahead_size(&self, n: u64) {
        self.readahead_size.store(n, Ordering::Relaxed);
    }

    /// Valide la configuration — retourne une erreur si un paramètre est hors limites.
    pub fn validate(&self) -> Result<(), ExofsError> {
        let gc_pct = self.gc_free_threshold_pct();
        if gc_pct == 0 || gc_pct >= 100 {
            return Err(ExofsError::InvalidArgument);
        }
        let compress_lvl = self.compress_level();
        if compress_lvl < 1 || compress_lvl > 9 {
            return Err(ExofsError::InvalidArgument);
        }
        let io_depth = self.io_queue_depth();
        if io_depth == 0 || io_depth > 1024 {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Profils de configuration
// ─────────────────────────────────────────────────────────────────────────────

/// Profil de configuration ExoFS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigProfile {
    /// Profil équilibré — défaut.
    Balanced,
    /// Profil performance — caches grands, GC moins fréquent, pas de vérif.
    Performance,
    /// Profil sécurité — checksum systématique, encryption requise, GC agressif.
    Safety,
    /// Profil dev/test — GC désactivé, caches petits.
    Development,
}

impl ConfigProfile {
    /// Applique le profil à la configuration.
    pub fn apply(&self, cfg: &ExofsConfig) {
        match self {
            ConfigProfile::Balanced => {
                // Déjà les valeurs par défaut.
            }
            ConfigProfile::Performance => {
                cfg.set_object_cache_size(16_384);
                cfg.set_blob_cache_size(32_768);
                cfg.set_path_cache_size(50_000);
                cfg.set_gc_timer_secs(300);
                cfg.set_gc_free_threshold_pct(10);
                cfg.set_writeback_interval_ms(5);
                cfg.set_readahead_size(1_048_576); // 1 MiB
                cfg.verify_checksums_on_read.store(0, Ordering::Relaxed);
            }
            ConfigProfile::Safety => {
                cfg.set_object_cache_size(2048);
                cfg.set_blob_cache_size(4096);
                cfg.set_gc_timer_secs(30);
                cfg.set_gc_free_threshold_pct(30);
                cfg.set_writeback_interval_ms(1);
                cfg.verify_checksums_on_read.store(1, Ordering::Relaxed);
                cfg.require_encryption_for_secrets
                    .store(1, Ordering::Relaxed);
                cfg.set_compress_level(6);
            }
            ConfigProfile::Development => {
                cfg.set_object_cache_size(256);
                cfg.set_blob_cache_size(512);
                cfg.set_path_cache_size(1000);
                cfg.set_gc_timer_secs(3600);
                cfg.set_gc_free_threshold_pct(5);
                cfg.set_readahead_size(0); // pas de readahead
                cfg.verify_checksums_on_read.store(1, Ordering::Relaxed);
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MountOptions — options de montage parsées
// ─────────────────────────────────────────────────────────────────────────────

/// Options de montage ExoFS (parsées depuis la ligne de commande kernel).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MountOptions {
    /// Montage en lecture seule.
    pub read_only: bool,
    /// Mode dégradé (montage malgré corruption mineure).
    pub degraded: bool,
    /// Profil de configuration à appliquer.
    pub profile: Option<ConfigProfile>,
    /// Désactive le GC automatique.
    pub no_gc: bool,
    /// Désactive la déduplication.
    pub no_dedup: bool,
    /// Désactive la compression.
    pub no_compress: bool,
    /// Active le mode debug.
    pub debug: bool,
    /// Taille de cache d'objets override (0 = utiliser le profil).
    pub object_cache_override: usize,
    /// Taille du readahead override en octets (0 = profil).
    pub readahead_override: u64,
}

impl MountOptions {
    /// Options de montage par défaut (read-write, profil balanced).
    pub const DEFAULT: Self = Self {
        read_only: false,
        degraded: false,
        profile: None,
        no_gc: false,
        no_dedup: false,
        no_compress: false,
        debug: false,
        object_cache_override: 0,
        readahead_override: 0,
    };

    /// Options de montage lecture seule.
    pub const READ_ONLY: Self = Self {
        read_only: true,
        ..Self::DEFAULT
    };

    /// Applique ces options à la configuration globale.
    pub fn apply(&self, cfg: &ExofsConfig) {
        // Profil en premier (définit les valeurs de base).
        if let Some(profile) = self.profile {
            profile.apply(cfg);
        }
        // Overrides spécifiques.
        if self.object_cache_override > 0 {
            cfg.set_object_cache_size(self.object_cache_override);
        }
        if self.readahead_override > 0 {
            cfg.set_readahead_size(self.readahead_override);
        }
        if self.no_gc {
            cfg.set_gc_timer_secs(u64::MAX);
        }
    }

    /// Retourne une erreur si les options sont incohérentes.
    pub fn validate(&self) -> Result<(), ExofsError> {
        // En mode read-only, les options d'écriture sont non pertinentes
        // mais pas invalides — on les ignore silencieusement.
        Ok(())
    }
}

/// Configuration globale ExoFS — initialisée au montage.
pub static EXOFS_CONFIG: ExofsConfig = ExofsConfig::default_config();

// ─────────────────────────────────────────────────────────────────────────────
// ConfigUpdate — delta de configuration pour hot-reload
// ─────────────────────────────────────────────────────────────────────────────

/// Champs modifiables via hot-reload (sans démontage).
///
/// Chaque variant correspond à un paramètre de `ExofsConfig` qui peut être
/// mis à jour à chaud sans impact sur l'intégrité du volume monté.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ConfigUpdate {
    ObjectCacheSize(usize),
    BlobCacheSize(usize),
    PathCacheSize(usize),
    GcFreeThresholdPct(u64),
    GcTimerSecs(u64),
    GcMinEpochDelay(u64),
    WritebackIntervalMs(u64),
    CompressMinSize(usize),
    DedupMinSize(usize),
}

impl ExofsConfig {
    /// Applique un `ConfigUpdate` à chaud (atomic store Relaxed → Release pour visibilité).
    ///
    /// Les mises à jour sont atomiques : aucune fenêtre d'inconsistance entre
    /// le lecteur et l'écrivain.
    pub fn apply_update(&self, update: ConfigUpdate) {
        match update {
            ConfigUpdate::ObjectCacheSize(v) => self.object_cache_size.store(v, Ordering::Release),
            ConfigUpdate::BlobCacheSize(v) => self.blob_cache_size.store(v, Ordering::Release),
            ConfigUpdate::PathCacheSize(v) => self.path_cache_size.store(v, Ordering::Release),
            ConfigUpdate::GcFreeThresholdPct(v) => {
                self.gc_free_threshold_pct.store(v, Ordering::Release)
            }
            ConfigUpdate::GcTimerSecs(v) => self.gc_timer_secs.store(v, Ordering::Release),
            ConfigUpdate::GcMinEpochDelay(v) => self.gc_min_epoch_delay.store(v, Ordering::Release),
            ConfigUpdate::WritebackIntervalMs(v) => {
                self.writeback_interval_ms.store(v, Ordering::Release)
            }
            ConfigUpdate::CompressMinSize(v) => self.compress_min_size.store(v, Ordering::Release),
            ConfigUpdate::DedupMinSize(v) => self.dedup_min_size.store(v, Ordering::Release),
        }
    }

    /// Applique un batch de mises à jour.
    ///
    /// Toutes les mises à jour sont appliquées séquentiellement.
    /// Si la validation d'une entrée échoue, les entrées déjà appliquées
    /// sont conservées (pas de rollback) — l'appelant doit valider en amont.
    pub fn apply_updates(&self, updates: &[ConfigUpdate]) {
        for &u in updates {
            self.apply_update(u);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ConfigBuilder — construction fluente d'une configuration de montage
// ─────────────────────────────────────────────────────────────────────────────

/// État intermédiaire pour construire une configuration avant montage.
///
/// Contrairement à `ExofsConfig` (atomique, statique), `ConfigBuilder` est
/// entièrement sur la pile et sans synchronisation. On l'utilise avant montage,
/// puis on l'applique à `EXOFS_CONFIG` via `ConfigBuilder::apply_to()`.
#[derive(Copy, Clone, Debug)]
pub struct ConfigBuilder {
    object_cache_size: usize,
    blob_cache_size: usize,
    path_cache_size: usize,
    gc_free_threshold_pct: u64,
    gc_timer_secs: u64,
    gc_min_epoch_delay: u64,
    writeback_interval_ms: u64,
    compress_min_size: usize,
    dedup_min_size: usize,
}

impl ConfigBuilder {
    /// Starts from the default configuration.
    pub const fn new() -> Self {
        Self {
            object_cache_size: 4096,
            blob_cache_size: 8192,
            path_cache_size: 2048,
            gc_free_threshold_pct: 20,
            gc_timer_secs: 30,
            gc_min_epoch_delay: 2,
            writeback_interval_ms: 500,
            compress_min_size: 4096,
            dedup_min_size: 4096,
        }
    }

    pub const fn object_cache_size(mut self, v: usize) -> Self {
        self.object_cache_size = v;
        self
    }
    pub const fn blob_cache_size(mut self, v: usize) -> Self {
        self.blob_cache_size = v;
        self
    }
    pub const fn path_cache_size(mut self, v: usize) -> Self {
        self.path_cache_size = v;
        self
    }
    pub const fn gc_free_threshold_pct(mut self, v: u64) -> Self {
        self.gc_free_threshold_pct = v;
        self
    }
    pub const fn gc_timer_secs(mut self, v: u64) -> Self {
        self.gc_timer_secs = v;
        self
    }
    pub const fn gc_min_epoch_delay(mut self, v: u64) -> Self {
        self.gc_min_epoch_delay = v;
        self
    }
    pub const fn writeback_interval_ms(mut self, v: u64) -> Self {
        self.writeback_interval_ms = v;
        self
    }
    pub const fn compress_min_size(mut self, v: usize) -> Self {
        self.compress_min_size = v;
        self
    }
    pub const fn dedup_min_size(mut self, v: usize) -> Self {
        self.dedup_min_size = v;
        self
    }

    /// Valide la cohérence interne.
    ///
    /// Retourne `Ok(())` si tous les paramètres sont dans des plages acceptables,
    /// ou une description d'erreur en cas de problème.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.object_cache_size == 0 {
            return Err("object_cache_size ne peut pas être nul");
        }
        if self.blob_cache_size == 0 {
            return Err("blob_cache_size ne peut pas être nul");
        }
        if self.gc_free_threshold_pct > 95 {
            return Err("gc_free_threshold_pct doit être ≤ 95");
        }
        if self.gc_timer_secs == 0 {
            return Err("gc_timer_secs ne peut pas être nul");
        }
        if self.writeback_interval_ms == 0 {
            return Err("writeback_interval_ms ne peut pas être nul");
        }
        Ok(())
    }

    /// Applique la configuration vers un `ExofsConfig` existant.
    pub fn apply_to(&self, cfg: &ExofsConfig) {
        cfg.apply_updates(&[
            ConfigUpdate::ObjectCacheSize(self.object_cache_size),
            ConfigUpdate::BlobCacheSize(self.blob_cache_size),
            ConfigUpdate::PathCacheSize(self.path_cache_size),
            ConfigUpdate::GcFreeThresholdPct(self.gc_free_threshold_pct),
            ConfigUpdate::GcTimerSecs(self.gc_timer_secs),
            ConfigUpdate::GcMinEpochDelay(self.gc_min_epoch_delay),
            ConfigUpdate::WritebackIntervalMs(self.writeback_interval_ms),
            ConfigUpdate::CompressMinSize(self.compress_min_size),
            ConfigUpdate::DedupMinSize(self.dedup_min_size),
        ]);
    }
}

impl Default for ConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ConfigSnapshot — capture instantanée non-atomique de la configuration
// ─────────────────────────────────────────────────────────────────────────────

/// Capture à un instant T de tous les paramètres de `ExofsConfig`.
///
/// La capture est effectuée avec `Ordering::Acquire` afin de garantir que
/// toutes les mises à jour antérieures utilisant `Ordering::Release` soient
/// visibles. Utile pour passer la config à du code qui travaille sur une vue
/// cohérente (ex. lors d'un montage ou d'un export de configuration).
#[derive(Copy, Clone, Debug)]
pub struct ConfigSnapshot {
    pub object_cache_size: usize,
    pub blob_cache_size: usize,
    pub path_cache_size: usize,
    pub gc_free_threshold_pct: u64,
    pub gc_timer_secs: u64,
    pub gc_min_epoch_delay: u64,
    pub writeback_interval_ms: u64,
    pub compress_min_size: usize,
    pub dedup_min_size: usize,
}

impl ExofsConfig {
    /// Prend un instantané cohérent de la configuration avec `Ordering::Acquire`.
    pub fn snapshot(&self) -> ConfigSnapshot {
        use core::sync::atomic::Ordering::Acquire;
        ConfigSnapshot {
            object_cache_size: self.object_cache_size.load(Acquire),
            blob_cache_size: self.blob_cache_size.load(Acquire),
            path_cache_size: self.path_cache_size.load(Acquire),
            gc_free_threshold_pct: self.gc_free_threshold_pct.load(Acquire),
            gc_timer_secs: self.gc_timer_secs.load(Acquire),
            gc_min_epoch_delay: self.gc_min_epoch_delay.load(Acquire),
            writeback_interval_ms: self.writeback_interval_ms.load(Acquire),
            compress_min_size: self.compress_min_size.load(Acquire),
            dedup_min_size: self.dedup_min_size.load(Acquire),
        }
    }
}

impl ConfigSnapshot {
    /// Retourne un `ConfigBuilder` initialisé depuis ce snapshot.
    ///
    /// Permet de modifier légèrement la config et de réappliquer :
    /// ```rust
    /// let snap = EXOFS_CONFIG.snapshot();
    /// snap.to_builder().gc_timer_secs(60).apply_to(&EXOFS_CONFIG);
    /// ```
    pub fn to_builder(self) -> ConfigBuilder {
        ConfigBuilder {
            object_cache_size: self.object_cache_size,
            blob_cache_size: self.blob_cache_size,
            path_cache_size: self.path_cache_size,
            gc_free_threshold_pct: self.gc_free_threshold_pct,
            gc_timer_secs: self.gc_timer_secs,
            gc_min_epoch_delay: self.gc_min_epoch_delay,
            writeback_interval_ms: self.writeback_interval_ms,
            compress_min_size: self.compress_min_size,
            dedup_min_size: self.dedup_min_size,
        }
    }
}
