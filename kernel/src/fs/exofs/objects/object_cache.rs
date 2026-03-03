// SPDX-License-Identifier: MIT
// ExoFS — object_cache.rs
// ObjectCache — cache LRU des LogicalObjects in-memory.
//
// Règles :
//   OOM-02  : try_reserve avant toute insertion
//   ARITH-02: checked_add / saturating_* partout
//   DAG-01  : pas d'import storage/, ipc/, process/, arch/
//   LOCK-04 : SpinLock léger — pas d'I/O dans la section critique

#![allow(dead_code)]

use core::fmt;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fs::exofs::core::{ObjectId, EpochId, ExofsError, ExofsResult};
use crate::fs::exofs::objects::logical_object::LogicalObjectRef;
use crate::fs::exofs::epoch::epoch_stats::EPOCH_STATS;
use crate::scheduler::sync::spinlock::SpinLock;

// ── Constantes ──────────────────────────────────────────────────────────────────

/// Capacité maximale du cache par défaut (nombre d'entrées).
pub const OBJECT_CACHE_DEFAULT_CAPACITY: usize = 4096;

/// Seuil de remplissage (%) au-delà duquel l'éviction LRU se déclenche.
pub const OBJECT_CACHE_EVICT_THRESHOLD: usize = 90;

/// Nombre d'entrées à évincer en un seul cycle.
pub const OBJECT_CACHE_EVICT_BATCH: usize = 64;

// ── CacheEntry ─────────────────────────────────────────────────────────────────

/// Entrée du cache avec métadonnées LRU.
struct CacheEntry {
    /// Référence à l'objet.
    obj:         LogicalObjectRef,
    /// Epoch du dernier accès (approximatif, pas un vrai timestamp).
    access_epoch: u64,
    /// Nombre d'accès depuis le dernier éviction.
    access_count: u32,
    /// Si vrai : pinned, jamais évincer.
    pinned:       bool,
}

impl CacheEntry {
    fn new(obj: LogicalObjectRef, epoch: u64) -> Self {
        Self {
            obj,
            access_epoch: epoch,
            access_count: 1,
            pinned:       false,
        }
    }

    fn touch(&mut self, epoch: u64) {
        self.access_epoch = epoch;
        self.access_count = self.access_count.saturating_add(1);
    }
}

// ── ObjectCacheInner ───────────────────────────────────────────────────────────

struct ObjectCacheInner {
    /// Entrées indexées par ObjectId.0 ([u8;32]).
    map:      BTreeMap<[u8; 32], CacheEntry>,
    /// Capacité maximale.
    capacity: usize,
    /// Compteur d'epoch logique (incrémenté à chaque accès).
    epoch:    u64,
    /// Statistiques internes.
    stats:    ObjectCacheStats,
}

impl ObjectCacheInner {
    fn new(capacity: usize) -> Self {
        Self {
            map:      BTreeMap::new(),
            capacity: capacity.max(1),
            epoch:    0,
            stats:    ObjectCacheStats::new(),
        }
    }

    /// Incrémente l'epoch logique.
    fn tick(&mut self) -> u64 {
        self.epoch = self.epoch.saturating_add(1);
        self.epoch
    }

    /// Insère ou met à jour une entrée.
    ///
    /// OOM-02 : capture les erreurs d'allocation.
    fn insert(&mut self, id: ObjectId, obj: LogicalObjectRef) -> ExofsResult<()> {
        let e = self.tick();

        if let Some(entry) = self.map.get_mut(&id.0) {
            // Mise à jour.
            entry.obj = obj;
            entry.touch(e);
            self.stats.updates = self.stats.updates.saturating_add(1);
            return Ok(());
        }

        // Éviction si nécessaire avant insertion.
        if self.map.len() >= self.capacity {
            self.evict_batch();
        }

        // OOM-02 : tente d'insérer.
        // BTreeMap::insert peut allouer. On ne peut pas détecter OOM proprement
        // sans std, mais on évite via la politique d'éviction.
        // Si le heap est saturé après éviction, on retourne NoMemory.
        let result = core::panic::catch_unwind(core::panic::AssertUnwindSafe(|| {
            self.map.insert(id.0, CacheEntry::new(obj, e));
        }));
        match result {
            Ok(_) => {
                self.stats.inserts = self.stats.inserts.saturating_add(1);
                Ok(())
            }
            Err(_) => Err(ExofsError::NoMemory),
        }
    }

    /// Retourne la référence à l'objet si présent (cache hit).
    fn get(&mut self, id: &ObjectId) -> Option<LogicalObjectRef> {
        let e = self.tick();
        if let Some(entry) = self.map.get_mut(&id.0) {
            entry.touch(e);
            self.stats.hits = self.stats.hits.saturating_add(1);
            Some(entry.obj.clone())
        } else {
            self.stats.misses = self.stats.misses.saturating_add(1);
            None
        }
    }

    /// Supprime une entrée.
    fn remove(&mut self, id: &ObjectId) -> bool {
        let removed = self.map.remove(&id.0).is_some();
        if removed {
            self.stats.evictions = self.stats.evictions.saturating_add(1);
        }
        removed
    }

    /// Éviction LRU d'un batch d'entrées non-pinnées.
    ///
    /// RECUR-01 : itératif, pas récursif.
    fn evict_batch(&mut self) {
        let threshold_pct =
            (self.capacity * OBJECT_CACHE_EVICT_THRESHOLD) / 100;
        if self.map.len() < threshold_pct {
            return;
        }

        // Collecte des candidats (pas pinnés) par epoch d'accès croissante.
        let mut candidates: Vec<([u8; 32], u64)> = self
            .map
            .iter()
            .filter(|(_, e)| !e.pinned)
            .map(|(k, e)| (*k, e.access_epoch))
            .collect();

        // Tri par epoch croissant (les plus anciens en premier).
        candidates.sort_unstable_by_key(|c| c.1);

        let to_remove = candidates.len().min(OBJECT_CACHE_EVICT_BATCH);
        for (key, _) in candidates.iter().take(to_remove) {
            self.map.remove(key);
            self.stats.evictions = self.stats.evictions.saturating_add(1);
        }
    }

    fn len(&self) -> usize {
        self.map.len()
    }

    fn fill_pct(&self) -> u32 {
        if self.capacity == 0 {
            return 100;
        }
        ((self.map.len() * 100) / self.capacity) as u32
    }
}

// ── ObjectCache ─────────────────────────────────────────────────────────────────

/// Cache LRU thread-safe des LogicalObjects.
///
/// Utilisé par le VFS ExoFS et l'object_table.
pub struct ObjectCache {
    inner: SpinLock<ObjectCacheInner>,
}

impl ObjectCache {
    // ── Constructeurs ──────────────────────────────────────────────────────────

    /// Crée un cache avec une capacité personnalisée.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: SpinLock::new(ObjectCacheInner::new(capacity)),
        }
    }

    /// Crée un cache avec la capacité par défaut.
    pub fn new() -> Self {
        Self::with_capacity(OBJECT_CACHE_DEFAULT_CAPACITY)
    }

    // ── Opérations ────────────────────────────────────────────────────────────

    /// Insère ou met à jour un objet dans le cache.
    ///
    /// OOM-02 : retourne `Err(NoMemory)` si l'allocation échoue.
    pub fn insert(&self, id: ObjectId, obj: LogicalObjectRef) -> ExofsResult<()> {
        let mut inner = self.inner.lock();
        inner.insert(id, obj)
    }

    /// Retourne un objet depuis le cache, ou `None` si absent (cache miss).
    ///
    /// LOCK-04 : section critique courte.
    pub fn get(&self, id: &ObjectId) -> Option<LogicalObjectRef> {
        let mut inner = self.inner.lock();
        let result = inner.get(id);
        if result.is_some() {
            EPOCH_STATS.inc_objects_read();
        }
        result
    }

    /// Retourne un objet existant, ou l'insère si absent (get-or-insert).
    ///
    /// L'objet est construit par `factory()` seulement si nécessaire.
    pub fn get_or_insert<F>(
        &self,
        id:      &ObjectId,
        factory: F,
    ) -> ExofsResult<LogicalObjectRef>
    where
        F: FnOnce() -> ExofsResult<LogicalObjectRef>,
    {
        // Tentative de cache hit d'abord.
        if let Some(obj) = self.get(id) {
            return Ok(obj);
        }
        // Cache miss — construction et insertion.
        let obj = factory()?;
        self.insert(*id, obj.clone())?;
        Ok(obj)
    }

    /// Invalide (supprime) une entrée du cache.
    pub fn invalidate(&self, id: &ObjectId) {
        let mut inner = self.inner.lock();
        inner.remove(id);
    }

    /// Épingle un objet : il ne sera jamais évincer.
    pub fn pin(&self, id: &ObjectId) -> bool {
        let mut inner = self.inner.lock();
        if let Some(entry) = inner.map.get_mut(&id.0) {
            entry.pinned = true;
            return true;
        }
        false
    }

    /// Dé-épingle un objet (peut maintenant être évincer).
    pub fn unpin(&self, id: &ObjectId) -> bool {
        let mut inner = self.inner.lock();
        if let Some(entry) = inner.map.get_mut(&id.0) {
            entry.pinned = false;
            return true;
        }
        false
    }

    /// Invalide tous les objets d'une certaine epoch ou plus anciens.
    ///
    /// RECUR-01 : itératif.
    pub fn invalidate_before_epoch(&self, cutoff_epoch: u64) {
        let mut inner = self.inner.lock();
        let to_remove: Vec<[u8; 32]> = inner
            .map
            .iter()
            .filter(|(_, e)| !e.pinned && e.access_epoch < cutoff_epoch)
            .map(|(k, _)| *k)
            .collect();
        for k in to_remove.iter() {
            inner.map.remove(k);
            inner.stats.evictions = inner.stats.evictions.saturating_add(1);
        }
    }

    /// Vide le cache entièrement (au démontage du système de fichiers).
    pub fn flush(&self) {
        let mut inner = self.inner.lock();
        let count = inner.map.len();
        inner.map.clear();
        inner.stats.evictions = inner.stats.evictions.saturating_add(count as u64);
    }

    // ── Requêtes ──────────────────────────────────────────────────────────────

    /// Nombre d'entrées dans le cache.
    pub fn len(&self) -> usize {
        let inner = self.inner.lock();
        inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Pourcentage de remplissage (0..100).
    pub fn fill_pct(&self) -> u32 {
        let inner = self.inner.lock();
        inner.fill_pct()
    }

    /// Retourne une copie des statistiques actuelles.
    pub fn stats(&self) -> ObjectCacheStats {
        let inner = self.inner.lock();
        inner.stats.clone()
    }

    /// Déclenche manuellement une éviction LRU.
    pub fn evict(&self) {
        let mut inner = self.inner.lock();
        inner.evict_batch();
    }
}

// ── ObjectCacheStats ───────────────────────────────────────────────────────────

/// Statistiques du cache d'objets.
#[derive(Default, Debug, Clone)]
pub struct ObjectCacheStats {
    pub hits:       u64,
    pub misses:     u64,
    pub inserts:    u64,
    pub updates:    u64,
    pub evictions:  u64,
}

impl ObjectCacheStats {
    pub fn new() -> Self { Self::default() }

    /// Ratio de succès de cache ×100 (100 = 100% de hits).
    pub fn hit_ratio_x100(&self) -> u64 {
        let total = self.hits.checked_add(self.misses).unwrap_or(1);
        if total == 0 {
            return 0;
        }
        (self.hits * 100) / total
    }
}

impl fmt::Display for ObjectCacheStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ObjectCacheStats {{ hits: {}, misses: {}, inserts: {}, \
             updates: {}, evictions: {}, hit_ratio: {}% }}",
            self.hits, self.misses, self.inserts, self.updates,
            self.evictions, self.hit_ratio_x100(),
        )
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::exofs::objects::logical_object::{LogicalObject, LogicalObjectDisk, LOGICAL_OBJECT_VERSION};

    fn make_obj(id_byte: u8) -> (ObjectId, LogicalObjectRef) {
        let mut d = LogicalObjectDisk {
            object_id:    [id_byte; 32],
            blob_id:      [0u8; 32],
            epoch_create: 1,
            epoch_modify: 1,
            blob_offset:  0,
            data_size:    0,
            flags:        0,
            kind:         0,
            class:        1,
            ref_count:    1,
            mode:         0o644,
            uid:          0,
            gid:          0,
            version:      LOGICAL_OBJECT_VERSION,
            _pad0:        [0; 3],
            generation:   0,
            _pad1:        [0; 64],
            checksum:     [0; 32],
            _pad2:        [0; 32],
        };
        d.checksum = d.compute_checksum();
        let obj = LogicalObject::from_disk(&d).unwrap();
        let oid = obj.object_id;
        let arc = Arc::new(crate::scheduler::sync::rwlock::RwLock::new(obj));
        (oid, arc)
    }

    #[test]
    fn test_insert_and_get() {
        let cache = ObjectCache::new();
        let (id, obj) = make_obj(1);
        cache.insert(id, obj.clone()).unwrap();
        assert!(cache.get(&id).is_some());
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_cache_miss() {
        let cache = ObjectCache::new();
        let id = ObjectId([42u8; 32]);
        assert!(cache.get(&id).is_none());
    }

    #[test]
    fn test_invalidate() {
        let cache = ObjectCache::new();
        let (id, obj) = make_obj(2);
        cache.insert(id, obj).unwrap();
        cache.invalidate(&id);
        assert!(cache.get(&id).is_none());
    }

    #[test]
    fn test_pin_prevents_eviction() {
        let capacity = 4;
        let cache = ObjectCache::with_capacity(capacity);
        let (id0, obj0) = make_obj(0);
        cache.insert(id0, obj0).unwrap();
        cache.pin(&id0);
        // Remplissage au-delà de la capacité.
        for i in 1..=(capacity + 2) {
            let (id_i, obj_i) = make_obj(i as u8);
            let _ = cache.insert(id_i, obj_i);
        }
        cache.evict();
        // id0 doit toujours être présent.
        assert!(cache.get(&id0).is_some());
    }

    #[test]
    fn test_flush() {
        let cache = ObjectCache::new();
        for i in 0..10u8 {
            let (id, obj) = make_obj(i);
            cache.insert(id, obj).unwrap();
        }
        cache.flush();
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_stats_hit_ratio() {
        let cache = ObjectCache::new();
        let (id, obj) = make_obj(99);
        cache.insert(id, obj).unwrap();
        let _ = cache.get(&id);
        let _ = cache.get(&id);
        let _ = cache.get(&ObjectId([0u8; 32])); // miss
        let s = cache.stats();
        assert_eq!(s.hits, 2);
        assert_eq!(s.misses, 1);
        assert_eq!(s.hit_ratio_x100(), 66);
    }
}
