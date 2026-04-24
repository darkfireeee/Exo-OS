//! relation_storage.rs — Stockage persistant des relations ExoFS
//!
//! Règles appliquées :
//!  - OOM-02   : try_reserve avant tout insert
//!  - ONDISK-03: AtomicU64 uniquement dans les structs non-repr(C)
//!  - ARITH-02 : arithmétique vérifiée

extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use super::relation::{Relation, RelationId, RelationOnDisk, RELATION_ONDISK_SIZE};
use super::relation_type::RelationKind;
use crate::fs::exofs::core::{BlobId, ExofsError, ExofsResult};
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximum de relations en mémoire simultanément.
pub const STORAGE_MAX_RELATIONS: usize = 65536;

// ─────────────────────────────────────────────────────────────────────────────
// StorageStats
// ─────────────────────────────────────────────────────────────────────────────

/// Statistiques du stockage de relations.
#[derive(Clone, Debug, Default)]
pub struct StorageStats {
    pub total_persisted: u64,
    pub total_removed: u64,
    pub current_count: usize,
    pub peak_count: usize,
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationStoreInner
// ─────────────────────────────────────────────────────────────────────────────

/// Partie intérieure du store protégée par SpinLock.
struct RelationStoreInner {
    /// Map id → blob on-disk.
    store: BTreeMap<u64, RelationOnDisk>,
    stats: StorageStats,
}

impl RelationStoreInner {
    const fn new_empty() -> Self {
        RelationStoreInner {
            store: BTreeMap::new(),
            stats: StorageStats {
                total_persisted: 0,
                total_removed: 0,
                current_count: 0,
                peak_count: 0,
            },
        }
    }

    fn persist(&mut self, rel: &Relation) -> ExofsResult<()> {
        if self.store.len() >= STORAGE_MAX_RELATIONS {
            return Err(ExofsError::NoSpace);
        }
        let on_disk = rel.to_on_disk();
        self.store.insert(rel.id.0, on_disk);
        self.stats.total_persisted = self.stats.total_persisted.wrapping_add(1);
        self.stats.current_count = self.store.len();
        if self.stats.current_count > self.stats.peak_count {
            self.stats.peak_count = self.stats.current_count;
        }
        Ok(())
    }

    fn remove(&mut self, id: u64) -> bool {
        if self.store.remove(&id).is_some() {
            self.stats.total_removed = self.stats.total_removed.wrapping_add(1);
            self.stats.current_count = self.store.len();
            true
        } else {
            false
        }
    }

    fn load(&self, id: u64) -> Option<ExofsResult<Relation>> {
        self.store.get(&id).map(Relation::from_on_disk)
    }

    fn load_all(&self) -> ExofsResult<Vec<Relation>> {
        let mut out = Vec::new();
        out.try_reserve(self.store.len())
            .map_err(|_| ExofsError::NoMemory)?;
        for d in self.store.values() {
            out.push(Relation::from_on_disk(d)?);
        }
        Ok(out)
    }

    fn load_by_kind(&self, kind: RelationKind) -> ExofsResult<Vec<Relation>> {
        let mut out = Vec::new();
        for d in self.store.values() {
            if d.kind == kind.to_u8() {
                out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                out.push(Relation::from_on_disk(d)?);
            }
        }
        Ok(out)
    }

    fn load_from(&self, from_blob: &[u8; 32]) -> ExofsResult<Vec<Relation>> {
        let mut out = Vec::new();
        for d in self.store.values() {
            if &d.from_blob == from_blob {
                out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                out.push(Relation::from_on_disk(d)?);
            }
        }
        Ok(out)
    }

    fn load_to(&self, to_blob: &[u8; 32]) -> ExofsResult<Vec<Relation>> {
        let mut out = Vec::new();
        for d in self.store.values() {
            if &d.to_blob == to_blob {
                out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                out.push(Relation::from_on_disk(d)?);
            }
        }
        Ok(out)
    }

    fn flush(&mut self) {
        self.store.clear();
        self.stats.current_count = 0;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationStorage (publique, thread-safe)
// ─────────────────────────────────────────────────────────────────────────────

/// Stockage persistant des relations en mémoire.
///
/// Thread-safe via SpinLock.  L'allocateur d'ID utilise un AtomicU64
/// en dehors de la zone repr(C) (ONDISK-03 ok).
pub struct RelationStorage {
    inner: SpinLock<RelationStoreInner>,
    next_id: AtomicU64,
}

impl RelationStorage {
    /// Constructeur `const` pour initialisation statique.
    pub const fn new_const() -> Self {
        RelationStorage {
            inner: SpinLock::new(RelationStoreInner::new_empty()),
            next_id: AtomicU64::new(1),
        }
    }

    /// Alloue un nouvel identifiant unique.
    pub fn allocate_id(&self) -> RelationId {
        let raw = self.next_id.fetch_add(1, Ordering::Relaxed);
        let raw = if raw == 0 {
            self.next_id.fetch_add(1, Ordering::Relaxed)
        } else {
            raw
        };
        RelationId(raw)
    }

    /// Persiste une relation.
    pub fn persist(&self, rel: &Relation) -> ExofsResult<()> {
        self.inner.lock().persist(rel)
    }

    /// Supprime une relation par son ID.  Retourne `false` si introuvable.
    pub fn remove(&self, id: RelationId) -> bool {
        self.inner.lock().remove(id.0)
    }

    /// Charge une relation par son ID.
    pub fn load(&self, id: RelationId) -> Option<ExofsResult<Relation>> {
        self.inner.lock().load(id.0)
    }

    /// Charge toutes les relations.
    pub fn load_all(&self) -> ExofsResult<Vec<Relation>> {
        self.inner.lock().load_all()
    }

    /// Charge les relations d'un certain type.
    pub fn load_by_kind(&self, kind: RelationKind) -> ExofsResult<Vec<Relation>> {
        self.inner.lock().load_by_kind(kind)
    }

    /// Charge les relations dont le blob source est `from_blob`.
    pub fn load_from(&self, from_blob: &BlobId) -> ExofsResult<Vec<Relation>> {
        self.inner.lock().load_from(from_blob.as_bytes())
    }

    /// Charge les relations dont la destination est `to_blob`.
    pub fn load_to(&self, to_blob: &BlobId) -> ExofsResult<Vec<Relation>> {
        self.inner.lock().load_to(to_blob.as_bytes())
    }

    /// `true` si une relation avec cet ID existe.
    pub fn contains(&self, id: RelationId) -> bool {
        self.inner.lock().store.contains_key(&id.0)
    }

    /// Nombre de relations stockées.
    pub fn count(&self) -> usize {
        self.inner.lock().store.len()
    }

    /// Statistiques du stockage.
    pub fn stats(&self) -> StorageStats {
        self.inner.lock().stats.clone()
    }

    /// Vide tout le store.
    pub fn flush(&self) {
        self.inner.lock().flush();
    }

    /// Itère sur tous les IDs stockés.
    pub fn all_ids(&self) -> Vec<RelationId> {
        let guard = self.inner.lock();
        let mut ids = Vec::new();
        for &k in guard.store.keys() {
            let _ = ids.try_reserve(1).map(|_| ids.push(RelationId(k)));
        }
        ids
    }

    /// Met à jour une relation existante (remplace par la nouvelle version).
    pub fn update(&self, rel: &Relation) -> ExofsResult<()> {
        let mut guard = self.inner.lock();
        if !guard.store.contains_key(&rel.id.0) {
            return Err(ExofsError::ObjectNotFound);
        }
        guard.store.insert(rel.id.0, rel.to_on_disk());
        Ok(())
    }

    /// Sérialise toutes les relations dans un Vec de blocs bruts.
    pub fn dump_raw(&self) -> ExofsResult<Vec<[u8; RELATION_ONDISK_SIZE]>> {
        let guard = self.inner.lock();
        let mut out = Vec::new();
        out.try_reserve(guard.store.len())
            .map_err(|_| ExofsError::NoMemory)?;
        for d in guard.store.values() {
            out.push(d.to_bytes());
        }
        Ok(out)
    }

    /// Recharge depuis des blocs bruts (import depuis disque).
    pub fn load_raw(&self, raw: &[[u8; RELATION_ONDISK_SIZE]]) -> ExofsResult<usize> {
        let mut count = 0usize;
        for buf in raw {
            let d = RelationOnDisk::from_bytes(buf)?;
            let rel = Relation::from_on_disk(&d)?;
            self.inner.lock().persist(&rel)?;
            count = count.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;
        }
        Ok(count)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Singleton global
// ─────────────────────────────────────────────────────────────────────────────

/// Store global des relations.
pub static RELATION_STORAGE: RelationStorage = RelationStorage::new_const();

// ─────────────────────────────────────────────────────────────────────────────
// Fonctions de commodité
// ─────────────────────────────────────────────────────────────────────────────

/// Alloue un ID et persiste la relation en une seule opération.
pub fn create_relation(
    from: BlobId,
    to: BlobId,
    rel_type: super::relation_type::RelationType,
    tick: u64,
) -> ExofsResult<Relation> {
    let id = RELATION_STORAGE.allocate_id();
    let rel = Relation::new(id, from, to, rel_type, tick);
    RELATION_STORAGE.persist(&rel)?;
    Ok(rel)
}

/// Purge toutes les relations soft-deleted du store.
pub fn purge_deleted() -> ExofsResult<usize> {
    let all = RELATION_STORAGE.load_all()?;
    let mut n = 0usize;
    for rel in all {
        if !rel.is_active() {
            RELATION_STORAGE.remove(rel.id);
            n = n.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;
        }
    }
    Ok(n)
}

/// Vérifie la cohérence minimale du store.
/// Retourne `true` si toutes les relations chargées sont parsables.
pub fn verify_store_integrity() -> bool {
    let raw = match RELATION_STORAGE.dump_raw() {
        Ok(r) => r,
        Err(_) => return false,
    };
    for buf in &raw {
        if RelationOnDisk::from_bytes(buf).is_err() {
            return false;
        }
        if !RelationOnDisk::crc_ok(buf) {
            return false;
        }
    }
    true
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::relation_type::RelationType;
    use super::*;

    fn blob(b: u8) -> BlobId {
        BlobId([b; 32])
    }

    fn make_rel(id: u64) -> Relation {
        Relation::new(
            RelationId(id),
            blob(1),
            blob(2),
            RelationType::new(RelationKind::Parent),
            1000,
        )
    }

    #[test]
    fn test_persist_load() {
        let store = RelationStorage::new_const();
        let rel = make_rel(1);
        store.persist(&rel).unwrap();
        let back = store.load(RelationId(1)).unwrap().unwrap();
        assert_eq!(back.id, RelationId(1));
    }

    #[test]
    fn test_remove() {
        let store = RelationStorage::new_const();
        store.persist(&make_rel(2)).unwrap();
        assert!(store.remove(RelationId(2)));
        assert!(!store.remove(RelationId(2)));
    }

    #[test]
    fn test_count() {
        let store = RelationStorage::new_const();
        store.persist(&make_rel(3)).unwrap();
        store.persist(&make_rel(4)).unwrap();
        assert_eq!(store.count(), 2);
    }

    #[test]
    fn test_load_all() {
        let store = RelationStorage::new_const();
        store.persist(&make_rel(5)).unwrap();
        store.persist(&make_rel(6)).unwrap();
        let all = store.load_all().unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_allocate_id_unique() {
        let store = RelationStorage::new_const();
        let id1 = store.allocate_id();
        let id2 = store.allocate_id();
        assert_ne!(id1, id2);
        assert!(id1.is_valid());
    }

    #[test]
    fn test_flush() {
        let store = RelationStorage::new_const();
        store.persist(&make_rel(10)).unwrap();
        store.flush();
        assert_eq!(store.count(), 0);
    }

    #[test]
    fn test_update() {
        let store = RelationStorage::new_const();
        let mut rel = make_rel(20);
        store.persist(&rel).unwrap();
        rel.mark_deleted(9999);
        store.update(&rel).unwrap();
        let back = store.load(RelationId(20)).unwrap().unwrap();
        assert!(!back.is_active());
    }

    #[test]
    fn test_load_by_kind() {
        let store = RelationStorage::new_const();
        store.persist(&make_rel(30)).unwrap();
        let mut rel2 = make_rel(31);
        rel2.rel_type = RelationType::new(RelationKind::Clone);
        store.persist(&rel2).unwrap();
        let parents = store.load_by_kind(RelationKind::Parent).unwrap();
        assert_eq!(parents.len(), 1);
    }
}
