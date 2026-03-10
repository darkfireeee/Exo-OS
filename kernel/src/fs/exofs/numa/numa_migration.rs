// SPDX-License-Identifier: MIT
// ExoFS NUMA — Migration de blobs entre nœuds NUMA
// ≥400L, ExofsError only, RECUR-01/OOM-02/ARITH-02

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::fs::exofs::core::{ExofsError, ExofsResult, BlobId};
use super::numa_affinity::MAX_NUMA_NODES;
use super::numa_stats::NUMA_STATS;

// ─── MigrationStatus ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MigrationStatus {
    /// Blob migré avec succès.
    Migrated,
    /// Blob déjà sur le nœud cible.
    AlreadyOnTarget,
    /// Blob non trouvé.
    NotFound,
    /// Nœud invalide.
    InvalidNode,
    /// Erreur d'I/O lors du déplacement.
    IoError,
    /// Pas de mémoire disponible.
    NoMemory,
    /// Migration annulée (nœud source sous pression).
    Cancelled,
}

impl MigrationStatus {
    pub fn is_success(self) -> bool { matches!(self, Self::Migrated | Self::AlreadyOnTarget) }
    pub fn is_error(self)   -> bool { !self.is_success() }
    pub fn name(self) -> &'static str {
        match self {
            Self::Migrated        => "migrated",
            Self::AlreadyOnTarget => "already-on-target",
            Self::NotFound        => "not-found",
            Self::InvalidNode     => "invalid-node",
            Self::IoError         => "io-error",
            Self::NoMemory        => "no-memory",
            Self::Cancelled       => "cancelled",
        }
    }
    pub fn to_exofs_error(self) -> ExofsError {
        match self {
            Self::NotFound        => ExofsError::BlobNotFound,
            Self::InvalidNode     => ExofsError::InvalidArgument,
            Self::IoError         => ExofsError::IoError,
            Self::NoMemory        => ExofsError::NoMemory,
            Self::Cancelled       => ExofsError::InternalError,
            Self::AlreadyOnTarget |
            Self::Migrated        => ExofsError::InternalError,
        }
    }
}

// ─── MigrationResult ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
pub struct MigrationResult {
    pub blob_id:  BlobId,
    pub from:     usize,
    pub to:       usize,
    pub bytes:    u64,
    pub status:   MigrationStatus,
    pub tick:     u64,
}

impl MigrationResult {
    pub fn success(blob_id: BlobId, from: usize, to: usize, bytes: u64, tick: u64) -> Self {
        Self { blob_id, from, to, bytes, status: MigrationStatus::Migrated, tick }
    }
    pub fn already_on_target(blob_id: BlobId, node: usize, tick: u64) -> Self {
        Self { blob_id, from: node, to: node, bytes: 0, status: MigrationStatus::AlreadyOnTarget, tick }
    }
    pub fn error(blob_id: BlobId, from: usize, to: usize, status: MigrationStatus, tick: u64) -> Self {
        Self { blob_id, from, to, bytes: 0, status, tick }
    }
    pub fn is_success(&self) -> bool { self.status.is_success() }
}

// ─── BlobNodeLocator (trait) ──────────────────────────────────────────────────

/// Trait permettant à `NumaMigration` de localiser et déplacer des blobs.
pub trait BlobNodeLocator {
    /// Retourne le nœud NUMA sur lequel réside le blob, ou None.
    fn node_of(&self, id: BlobId) -> Option<usize>;
    /// Retourne la taille en octets du blob, ou None.
    fn byte_size(&self, id: BlobId) -> Option<u64>;
    /// Déplace le blob vers le nœud cible.
    fn move_to_node(&self, id: BlobId, target: usize) -> ExofsResult<()>;
}

// ─── MigrationPolicy ─────────────────────────────────────────────────────────

/// Politique de migration (seuils de déclenchement).
#[derive(Clone, Copy, Debug)]
pub struct MigrationPolicy {
    /// Déséquilibre minimal en ‰ pour déclencher une migration automatique.
    pub imbalance_trigger_ppt: u64,
    /// Nombre max de blobs simultanément en migration.
    pub max_concurrent:        usize,
    /// Taille minimale d'un blob pour migration (évite de migrer les petits blobs).
    pub min_blob_bytes:        u64,
    /// Active les migrations automatiques.
    pub auto_enabled:          bool,
}

impl MigrationPolicy {
    pub const fn default_policy() -> Self {
        Self {
            imbalance_trigger_ppt: 200,
            max_concurrent:        8,
            min_blob_bytes:        4096,
            auto_enabled:          false,
        }
    }
    pub fn validate(&self) -> ExofsResult<()> {
        if self.max_concurrent == 0 { return Err(ExofsError::InvalidArgument); }
        Ok(())
    }
}

// ─── MigrationQueue ───────────────────────────────────────────────────────────

/// Registre des migrations en cours (tableau plat, max 64 entrées).
pub const MIGRATION_QUEUE_MAX: usize = 64;

#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
struct MigrationSlot {
    blob_id: BlobId,
    from:    u8,
    to:      u8,
    active:  bool,
}

impl MigrationSlot {
    const fn empty() -> Self {
        Self { blob_id: BlobId([0u8; 32]), from: 0, to: 0, active: false }
    }
}

pub struct MigrationQueue {
    slots: core::cell::UnsafeCell<[MigrationSlot; MIGRATION_QUEUE_MAX]>,
    count: AtomicU64,
    lock:  AtomicU64,
}

unsafe impl Sync for MigrationQueue {}
unsafe impl Send for MigrationQueue {}

impl MigrationQueue {
    pub const fn new_const() -> Self {
        Self {
            slots: core::cell::UnsafeCell::new([MigrationSlot::empty(); MIGRATION_QUEUE_MAX]),
            count: AtomicU64::new(0),
            lock:  AtomicU64::new(0),
        }
    }
    fn acquire(&self) {
        while self.lock.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() {
            core::hint::spin_loop();
        }
    }
    fn release(&self) { self.lock.store(0, Ordering::Release); }

    /// Ajoute un blob en cours de migration.
    pub fn push(&self, blob_id: BlobId, from: usize, to: usize) -> ExofsResult<()> {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let slots = unsafe { &mut *self.slots.get() };
        let mut i = 0usize;
        while i < MIGRATION_QUEUE_MAX {
            if !slots[i].active {
                slots[i] = MigrationSlot {
                    blob_id, from: from as u8, to: to as u8, active: true
                };
                self.count.fetch_add(1, Ordering::Relaxed);
                self.release();
                return Ok(());
            }
            i = i.wrapping_add(1);
        }
        self.release();
        Err(ExofsError::GcQueueFull)
    }

    /// Retire un blob de la file de migration.
    pub fn remove(&self, blob_id: BlobId) {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let slots = unsafe { &mut *self.slots.get() };
        let mut i = 0usize;
        while i < MIGRATION_QUEUE_MAX {
            if slots[i].active && slots[i].blob_id.as_bytes() == blob_id.as_bytes() {
                slots[i].active = false;
                self.count.fetch_sub(1, Ordering::Relaxed);
                break;
            }
            i = i.wrapping_add(1);
        }
        self.release();
    }

    /// Vrai si un blob est déjà en cours de migration.
    pub fn contains(&self, blob_id: BlobId) -> bool {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let slots = unsafe { &*self.slots.get() };
        let mut found = false;
        let mut i = 0usize;
        while i < MIGRATION_QUEUE_MAX {
            if slots[i].active && slots[i].blob_id.as_bytes() == blob_id.as_bytes() {
                found = true;
                break;
            }
            i = i.wrapping_add(1);
        }
        self.release();
        found
    }

    pub fn active_count(&self) -> usize { self.count.load(Ordering::Relaxed) as usize }
}

// ─── NumaMigration ────────────────────────────────────────────────────────────

/// Moteur de migration NUMA.
pub struct NumaMigration {
    policy:           core::cell::UnsafeCell<MigrationPolicy>,
    queue:            MigrationQueue,
    total_migrated:   AtomicU64,
    total_bytes:      AtomicU64,
    total_errors:     AtomicU64,
    lock:             AtomicU64,
}

unsafe impl Sync for NumaMigration {}
unsafe impl Send for NumaMigration {}

impl NumaMigration {
    pub const fn new_const() -> Self {
        Self {
            policy:         core::cell::UnsafeCell::new(MigrationPolicy::default_policy()),
            queue:          MigrationQueue::new_const(),
            total_migrated: AtomicU64::new(0),
            total_bytes:    AtomicU64::new(0),
            total_errors:   AtomicU64::new(0),
            lock:           AtomicU64::new(0),
        }
    }

    fn acquire(&self) {
        while self.lock.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() {
            core::hint::spin_loop();
        }
    }
    fn release(&self) { self.lock.store(0, Ordering::Release); }

    pub fn configure(&self, policy: MigrationPolicy) -> ExofsResult<()> {
        policy.validate()?;
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        unsafe { *self.policy.get() = policy; }
        self.release();
        Ok(())
    }

    pub fn policy(&self) -> MigrationPolicy {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let p = unsafe { *self.policy.get() };
        self.release();
        p
    }

    /// Migre un blob vers un nœud cible.
    pub fn migrate_blob(
        &self,
        locator:     &dyn BlobNodeLocator,
        id:          BlobId,
        target_node: usize,
        tick:        u64,
    ) -> MigrationResult {
        // Validation du nœud cible
        if target_node >= MAX_NUMA_NODES {
            self.total_errors.fetch_add(1, Ordering::Relaxed);
            return MigrationResult::error(id, 0, target_node, MigrationStatus::InvalidNode, tick);
        }

        // Localiser le blob
        let from_node = match locator.node_of(id) {
            Some(n) => n,
            None => {
                self.total_errors.fetch_add(1, Ordering::Relaxed);
                return MigrationResult::error(id, 0, target_node, MigrationStatus::NotFound, tick);
            }
        };

        if from_node == target_node {
            return MigrationResult::already_on_target(id, from_node, tick);
        }

        // Vérification double migration
        if self.queue.contains(id) {
            self.total_errors.fetch_add(1, Ordering::Relaxed);
            return MigrationResult::error(id, from_node, target_node, MigrationStatus::Cancelled, tick);
        }

        let policy = self.policy();
        // Vérifier la limite concurrente (ARITH-02)
        if self.queue.active_count() >= policy.max_concurrent {
            self.total_errors.fetch_add(1, Ordering::Relaxed);
            return MigrationResult::error(id, from_node, target_node, MigrationStatus::Cancelled, tick);
        }

        let bytes = locator.byte_size(id).unwrap_or(0);

        // Filtre taille minimale (ARITH-02 : comparaison simple)
        if bytes < policy.min_blob_bytes && bytes > 0 {
            return MigrationResult::already_on_target(id, from_node, tick);
        }

        // Enregistrer en file
        if self.queue.push(id, from_node, target_node).is_err() {
            self.total_errors.fetch_add(1, Ordering::Relaxed);
            return MigrationResult::error(id, from_node, target_node, MigrationStatus::NoMemory, tick);
        }

        // Appel effectif
        let result = locator.move_to_node(id, target_node);
        self.queue.remove(id);

        match result {
            Ok(()) => {
                // Mettre à jour les statistiques
                NUMA_STATS.record_free(from_node, bytes);
                NUMA_STATS.record_alloc(target_node, bytes);
                NUMA_STATS.record_migration(from_node, target_node);
                self.total_migrated.fetch_add(1, Ordering::Relaxed);
                self.total_bytes.fetch_add(bytes, Ordering::Relaxed);
                MigrationResult::success(id, from_node, target_node, bytes, tick)
            }
            Err(e) => {
                self.total_errors.fetch_add(1, Ordering::Relaxed);
                let reason = if e == ExofsError::NoMemory { MigrationStatus::NoMemory }
                             else { MigrationStatus::IoError };
                MigrationResult::error(id, from_node, target_node, reason, tick)
            }
        }
    }

    /// Migre un lot de blobs vers les nœuds les moins chargés (RECUR-01, OOM-02).
    pub fn rebalance(
        &self,
        locator:  &dyn BlobNodeLocator,
        blob_ids: &[BlobId],
        tick:     u64,
    ) -> ExofsResult<Vec<MigrationResult>> {
        let n = blob_ids.len();
        let mut results = Vec::new();
        results.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;

        let mut i = 0usize;
        while i < n {
            let id = blob_ids[i];
            // Cibler le nœud le moins chargé
            let target = NUMA_STATS.least_loaded_node();
            let r = self.migrate_blob(locator, id, target, tick);
            results.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            results.push(r);
            i = i.wrapping_add(1);
        }
        Ok(results)
    }

    pub fn total_migrated(&self) -> u64  { self.total_migrated.load(Ordering::Relaxed) }
    pub fn total_bytes(&self) -> u64     { self.total_bytes.load(Ordering::Relaxed) }
    pub fn total_errors(&self) -> u64    { self.total_errors.load(Ordering::Relaxed) }
    pub fn is_healthy(&self) -> bool     { self.total_errors() == 0 }
    pub fn active_migrations(&self) -> usize { self.queue.active_count() }

    pub fn reset_stats(&self) {
        self.total_migrated.store(0, Ordering::Relaxed);
        self.total_bytes.store(0, Ordering::Relaxed);
        self.total_errors.store(0, Ordering::Relaxed);
    }
}

/// Singleton global du moteur de migration.
pub static NUMA_MIGRATION: NumaMigration = NumaMigration::new_const();

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    struct MockLocator;

    impl BlobNodeLocator for MockLocator {
        fn node_of(&self, _id: BlobId) -> Option<usize> { Some(0) }
        fn byte_size(&self, _id: BlobId) -> Option<u64>  { Some(8192) }
        fn move_to_node(&self, _id: BlobId, _target: usize) -> ExofsResult<()> { Ok(()) }
    }

    struct FailLocator;

    impl BlobNodeLocator for FailLocator {
        fn node_of(&self, _id: BlobId) -> Option<usize> { Some(1) }
        fn byte_size(&self, _id: BlobId) -> Option<u64>  { Some(8192) }
        fn move_to_node(&self, _id: BlobId, _target: usize) -> ExofsResult<()> {
            Err(ExofsError::IoError)
        }
    }

    struct NoneLocator;

    impl BlobNodeLocator for NoneLocator {
        fn node_of(&self, _id: BlobId) -> Option<usize> { None }
        fn byte_size(&self, _id: BlobId) -> Option<u64>  { None }
        fn move_to_node(&self, _id: BlobId, _target: usize) -> ExofsResult<()> {
            Err(ExofsError::BlobNotFound)
        }
    }

    fn zero_blob() -> BlobId { BlobId([0u8; 32]) }
    fn one_blob()  -> BlobId { BlobId([1u8; 32]) }

    #[test]
    fn test_migrate_success() {
        let m = NumaMigration::new_const();
        let r = m.migrate_blob(&MockLocator, zero_blob(), 1, 0);
        assert!(r.is_success());
        assert_eq!(r.status, MigrationStatus::Migrated);
        assert_eq!(r.from, 0);
        assert_eq!(r.to, 1);
        assert_eq!(r.bytes, 8192);
    }

    #[test]
    fn test_migrate_already_on_target() {
        let m = NumaMigration::new_const();
        // MockLocator retourne node=0, target=0
        let r = m.migrate_blob(&MockLocator, zero_blob(), 0, 0);
        assert_eq!(r.status, MigrationStatus::AlreadyOnTarget);
        assert!(r.is_success());
    }

    #[test]
    fn test_migrate_not_found() {
        let m = NumaMigration::new_const();
        let r = m.migrate_blob(&NoneLocator, zero_blob(), 1, 0);
        assert_eq!(r.status, MigrationStatus::NotFound);
        assert!(r.is_error());
    }

    #[test]
    fn test_migrate_invalid_node() {
        let m = NumaMigration::new_const();
        let r = m.migrate_blob(&MockLocator, zero_blob(), 99, 0);
        assert_eq!(r.status, MigrationStatus::InvalidNode);
    }

    #[test]
    fn test_migrate_io_error() {
        let m = NumaMigration::new_const();
        let r = m.migrate_blob(&FailLocator, zero_blob(), 2, 0);
        assert_eq!(r.status, MigrationStatus::IoError);
        assert_eq!(m.total_errors(), 1);
    }

    #[test]
    fn test_migrate_stats_updated() {
        let m = NumaMigration::new_const();
        NUMA_STATS.reset_all();
        m.migrate_blob(&MockLocator, zero_blob(), 1, 0);
        assert_eq!(m.total_migrated(), 1);
        assert_eq!(m.total_bytes(), 8192);
        assert_eq!(m.total_errors(), 0);
    }

    #[test]
    fn test_policy_configure() {
        let m = NumaMigration::new_const();
        let p = MigrationPolicy { max_concurrent: 2, min_blob_bytes: 0,
                                  imbalance_trigger_ppt: 100, auto_enabled: true };
        m.configure(p).unwrap();
        assert_eq!(m.policy().max_concurrent, 2);
    }

    #[test]
    fn test_policy_zero_concurrent_error() {
        let m = NumaMigration::new_const();
        let p = MigrationPolicy { max_concurrent: 0, min_blob_bytes: 0,
                                  imbalance_trigger_ppt: 100, auto_enabled: false };
        assert!(m.configure(p).is_err());
    }

    #[test]
    fn test_rebalance_empty_list() {
        let m = NumaMigration::new_const();
        let results = m.rebalance(&MockLocator, &[], 0).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_rebalance_single_blob() {
        let m = NumaMigration::new_const();
        let blobs = [zero_blob()];
        let results = m.rebalance(&MockLocator, &blobs, 0).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_status_name() {
        assert_eq!(MigrationStatus::Migrated.name(),    "migrated");
        assert_eq!(MigrationStatus::NotFound.name(),    "not-found");
        assert_eq!(MigrationStatus::InvalidNode.name(), "invalid-node");
    }

    #[test]
    fn test_reset_stats() {
        let m = NumaMigration::new_const();
        m.migrate_blob(&MockLocator, zero_blob(), 1, 0);
        m.reset_stats();
        assert_eq!(m.total_migrated(), 0);
        assert_eq!(m.total_bytes(), 0);
    }
}
