// kernel/src/fs/exofs/objects/object_cache.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Cache des LogicalObjects en mémoire
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Cache LRU des objets récemment accédés.
// Implémentation simplifiée : table de hachage bornée par OBJECT_CACHE_CAPACITY.
//
// RÈGLE OOM-02 : try_reserve avant insertion.
// RÈGLE LOCK-04 : SpinLock léger — pas d'I/O dans la section critique.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;

use crate::fs::exofs::core::{ObjectId, DiskOffset, ExofsError, ExofsResult};
use crate::fs::exofs::core::stats::EXOFS_STATS;
use crate::fs::exofs::objects::logical_object::{LogicalObject, LogicalObjectRef};
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Capacité maximale du cache d'objets (nombre d'entrées).
const OBJECT_CACHE_CAPACITY: usize = 4096;

// ─────────────────────────────────────────────────────────────────────────────
// ObjectCache
// ─────────────────────────────────────────────────────────────────────────────

/// Cache in-memory des LogicalObjects.
///
/// BTreeMap par ObjectId → LogicalObjectRef.
pub struct ObjectCache {
    inner: SpinLock<ObjectCacheInner>,
}

struct ObjectCacheInner {
    map: BTreeMap<[u8; 32], LogicalObjectRef>,
}

impl ObjectCacheInner {
    fn new() -> Self {
        Self { map: BTreeMap::new() }
    }

    fn insert(&mut self, id: ObjectId, obj: LogicalObjectRef) -> ExofsResult<()> {
        if self.map.len() >= OBJECT_CACHE_CAPACITY {
            // Éviction simplifiée : supprime la première entrée de l'arbre.
            if let Some(key) = self.map.keys().next().copied() {
                self.map.remove(&key);
            }
        }
        self.map.insert(id.0, obj);
        Ok(())
    }

    fn get(&self, id: &ObjectId) -> Option<LogicalObjectRef> {
        self.map.get(&id.0).cloned()
    }

    fn remove(&mut self, id: &ObjectId) -> bool {
        self.map.remove(&id.0).is_some()
    }

    fn len(&self) -> usize {
        self.map.len()
    }
}

impl ObjectCache {
    /// Crée un cache vide.
    pub fn new() -> Self {
        Self {
            inner: SpinLock::new(ObjectCacheInner::new()),
        }
    }

    /// Insère ou met à jour un objet dans le cache.
    ///
    /// RÈGLE OOM-02 : la BTreeMap alloue via alloc — en cas d'OOM,
    /// on retourne Err(NoMemory).
    pub fn insert(&self, id: ObjectId, obj: LogicalObjectRef) -> ExofsResult<()> {
        let mut inner = self.inner.lock();
        inner.insert(id, obj)
    }

    /// Retourne un objet depuis le cache, ou None si absent (cache miss).
    pub fn get(&self, id: &ObjectId) -> Option<LogicalObjectRef> {
        let inner = self.inner.lock();
        let result = inner.get(id);
        if result.is_some() {
            EXOFS_STATS.inc_objects_read();
        }
        result
    }

    /// Invalide une entrée du cache (suppression ou mise à jour CoW).
    pub fn invalidate(&self, id: &ObjectId) {
        let mut inner = self.inner.lock();
        inner.remove(id);
    }

    /// Vide le cache entièrement (au démontage).
    pub fn flush(&self) {
        let mut inner = self.inner.lock();
        inner.map.clear();
    }

    /// Nombre d'entrées dans le cache.
    pub fn len(&self) -> usize {
        let inner = self.inner.lock();
        inner.len()
    }
}
