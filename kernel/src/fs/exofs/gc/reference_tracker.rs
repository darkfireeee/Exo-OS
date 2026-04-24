// kernel/src/fs/exofs/gc/reference_tracker.rs
//
// ==============================================================================
// Tracker de references ObjetId -> BlobId pour le GC ExoFS
// Ring 0 . no_std . Exo-OS
//
// Maintient le graphe de references entre objects logiques et P-Blobs :
//   ObjectId -> Vec<BlobId>  (blobs directs d'un objet)
//   BlobId   -> Vec<BlobId>  (sous-blobs : extents, metadata blobs)
//
// Utilise par le marker pour traverser le graphe et le cycle_detector.
//
// Conformite :
//   GC-02 : traversee complete incluant les sous-blobs
//   RECUR-01 : toutes les traversees sont iteratives (stack heap-allouee)
//   OOM-02 : try_reserve avant chaque insertion
//   ARITH-02 : checked_add sur les compteurs
// ==============================================================================

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::fmt;

use crate::fs::exofs::core::{BlobId, ExofsError, ExofsResult, ObjectId};
use crate::scheduler::sync::spinlock::SpinLock;

// ==============================================================================
// Constantes
// ==============================================================================

/// Nombre maximum de references par objet.
pub const MAX_REFS_PER_OBJECT: usize = 64;

/// Nombre maximum de sous-blobs par blob.
pub const MAX_SUBBLOBS_PER_BLOB: usize = 32;

/// Capacite initiale du graphe de references.
pub const REF_GRAPH_INITIAL_CAP: usize = 1024;

// ==============================================================================
// RefGraphStats — statistiques du tracker
// ==============================================================================

/// Statistiques du graphe de references.
#[derive(Debug, Default, Clone)]
pub struct RefGraphStats {
    /// Nombre d'objets suivis.
    pub objects_tracked: u64,
    /// Nombre total de references objet->blob enregistrees.
    pub obj_blob_refs: u64,
    /// Nombre de blobs suivis avec sous-blobs.
    pub blobs_tracked: u64,
    /// Nombre total de sous-references blob->blob.
    pub blob_blob_refs: u64,
    /// Lectures (traversees) effectuees.
    pub traversals: u64,
}

impl fmt::Display for RefGraphStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "RefGraph[objs={} obj->blob={} blobs={} blob->blob={} traversals={}]",
            self.objects_tracked,
            self.obj_blob_refs,
            self.blobs_tracked,
            self.blob_blob_refs,
            self.traversals,
        )
    }
}

// ==============================================================================
// ReferenceTrackerInner — donnees protegees
// ==============================================================================

struct ReferenceTrackerInner {
    /// Graphe : ObjectId -> liste de BlobId directement references.
    obj_to_blobs: BTreeMap<ObjectId, Vec<BlobId>>,
    /// Graphe : BlobId -> liste de sous-BlobId (extents, meta-blobs).
    blob_to_subs: BTreeMap<BlobId, Vec<BlobId>>,
    /// Statistiques.
    stats: RefGraphStats,
}

impl ReferenceTrackerInner {
    #[allow(dead_code)]
    fn new() -> Self {
        Self {
            obj_to_blobs: BTreeMap::new(),
            blob_to_subs: BTreeMap::new(),
            stats: RefGraphStats::default(),
        }
    }
}

// ==============================================================================
// ReferenceTracker — facade thread-safe
// ==============================================================================

/// Graphe de references pour le GC.
///
/// Thread-safe via SpinLock.
/// DEAD-01 : n'acquiert jamais EPOCH_COMMIT_LOCK.
pub struct ReferenceTracker {
    inner: SpinLock<ReferenceTrackerInner>,
}

impl ReferenceTracker {
    pub const fn new() -> Self {
        Self {
            inner: SpinLock::new(ReferenceTrackerInner {
                obj_to_blobs: BTreeMap::new(),
                blob_to_subs: BTreeMap::new(),
                stats: RefGraphStats {
                    objects_tracked: 0,
                    obj_blob_refs: 0,
                    blobs_tracked: 0,
                    blob_blob_refs: 0,
                    traversals: 0,
                },
            }),
        }
    }

    // ── Enregistrement ───────────────────────────────────────────────────────

    /// Enregistre une reference ObjectId -> BlobId.
    ///
    /// OOM-02 : try_reserve avant push.
    pub fn add_obj_ref(&self, obj: ObjectId, blob: BlobId) -> ExofsResult<()> {
        let mut g = self.inner.lock();
        let refs = g.obj_to_blobs.entry(obj).or_insert_with(Vec::new);

        if refs.len() >= MAX_REFS_PER_OBJECT {
            return Err(ExofsError::Resource);
        }

        refs.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        // Evite les doublons.
        if !refs.contains(&blob) {
            refs.push(blob);
            g.stats.obj_blob_refs = g.stats.obj_blob_refs.saturating_add(1);
            if g.obj_to_blobs.len() as u64 > g.stats.objects_tracked {
                g.stats.objects_tracked = g.obj_to_blobs.len() as u64;
            }
        }
        Ok(())
    }

    /// Enregistre une reference BlobId -> sous-BlobId.
    ///
    /// OOM-02 : try_reserve avant push.
    pub fn add_blob_ref(&self, parent: BlobId, child: BlobId) -> ExofsResult<()> {
        let mut g = self.inner.lock();
        let refs = g.blob_to_subs.entry(parent).or_insert_with(Vec::new);

        if refs.len() >= MAX_SUBBLOBS_PER_BLOB {
            return Err(ExofsError::Resource);
        }

        refs.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        if !refs.contains(&child) {
            refs.push(child);
            g.stats.blob_blob_refs = g.stats.blob_blob_refs.saturating_add(1);
            if g.blob_to_subs.len() as u64 > g.stats.blobs_tracked {
                g.stats.blobs_tracked = g.blob_to_subs.len() as u64;
            }
        }
        Ok(())
    }

    // ── Suppression ──────────────────────────────────────────────────────────

    /// Supprime toutes les references d'un objet.
    pub fn remove_obj(&self, obj: &ObjectId) {
        let mut g = self.inner.lock();
        if let Some(refs) = g.obj_to_blobs.remove(obj) {
            g.stats.obj_blob_refs = g.stats.obj_blob_refs.saturating_sub(refs.len() as u64);
        }
    }

    /// Supprime toutes les sous-references d'un blob.
    pub fn remove_blob(&self, blob: &BlobId) {
        let mut g = self.inner.lock();
        if let Some(refs) = g.blob_to_subs.remove(blob) {
            g.stats.blob_blob_refs = g.stats.blob_blob_refs.saturating_sub(refs.len() as u64);
        }
    }

    // ── Lecture ──────────────────────────────────────────────────────────────

    /// Retourne les BlobIds directement references par un ObjectId.
    pub fn get_obj_refs(&self, obj: &ObjectId) -> Vec<BlobId> {
        let mut g = self.inner.lock();
        g.stats.traversals = g.stats.traversals.saturating_add(1);
        g.obj_to_blobs.get(obj).cloned().unwrap_or_default()
    }

    /// Retourne les sous-BlobIds d'un blob.
    pub fn get_refs(&self, blob: &BlobId) -> Vec<BlobId> {
        let mut g = self.inner.lock();
        g.stats.traversals = g.stats.traversals.saturating_add(1);
        g.blob_to_subs.get(blob).cloned().unwrap_or_default()
    }

    /// Collecte tous les BlobIds atteignables depuis un ObjectId.
    ///
    /// RECUR-01 : iteratif avec pile heap-allouee.
    /// Traverse : obj -> blobs directs -> sous-blobs (transitivement).
    pub fn all_reachable_blobs(&self, obj: &ObjectId) -> ExofsResult<Vec<BlobId>> {
        let mut result: Vec<BlobId> = Vec::new();
        // Pile iterative pour traversee DFS.
        let mut stack: Vec<BlobId> = Vec::new();
        let mut visited: alloc::collections::BTreeSet<BlobId> = alloc::collections::BTreeSet::new();

        // Amorcer depuis les references directes de l'objet.
        let direct = self.get_obj_refs(obj);
        for b in direct {
            if visited.insert(b) {
                stack.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                stack.push(b);
            }
        }

        // Traversee iterative (RECUR-01).
        while let Some(current) = stack.pop() {
            result.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            result.push(current);

            let children = self.get_refs(&current);
            for child in children {
                if visited.insert(child) {
                    stack.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    stack.push(child);
                }
            }
        }

        Ok(result)
    }

    /// Collecte toutes les racines (ObjectIds) connues.
    pub fn all_objects(&self) -> Vec<ObjectId> {
        self.inner.lock().obj_to_blobs.keys().copied().collect()
    }

    /// Collecte tous les BlobIds connus.
    pub fn all_blobs(&self) -> Vec<BlobId> {
        let g = self.inner.lock();
        // Union des blobs directs et des blobs avec sous-refs.
        let mut blobs: alloc::collections::BTreeSet<BlobId> = alloc::collections::BTreeSet::new();
        for refs in g.obj_to_blobs.values() {
            blobs.extend(refs.iter().copied());
        }
        for (b, subs) in g.blob_to_subs.iter() {
            blobs.insert(*b);
            blobs.extend(subs.iter().copied());
        }
        blobs.into_iter().collect()
    }

    // ── Reset ────────────────────────────────────────────────────────────────

    /// Efface completement le graphe (apres une passe GC).
    pub fn clear(&self) {
        let mut g = self.inner.lock();
        g.obj_to_blobs.clear();
        g.blob_to_subs.clear();
        g.stats.obj_blob_refs = 0;
        g.stats.blob_blob_refs = 0;
    }

    /// Statistiques courantes.
    pub fn stats(&self) -> RefGraphStats {
        self.inner.lock().stats.clone()
    }

    /// Nombre d'objets suivis.
    pub fn object_count(&self) -> usize {
        self.inner.lock().obj_to_blobs.len()
    }

    /// Nombre de blobs avec sous-references.
    pub fn blob_count(&self) -> usize {
        self.inner.lock().blob_to_subs.len()
    }
}

// ==============================================================================
// Instance globale
// ==============================================================================

/// Tracker de references global.
pub static REFERENCE_TRACKER: ReferenceTracker = ReferenceTracker::new();

// ==============================================================================
// Tests
// ==============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn oid(b: u8) -> ObjectId {
        let mut arr = [0u8; 32];
        arr[0] = b;
        ObjectId(arr)
    }

    fn bid(b: u8) -> BlobId {
        let mut arr = [0u8; 32];
        arr[0] = b;
        BlobId(arr)
    }

    #[test]
    fn test_add_and_get_obj_ref() {
        let t = ReferenceTracker::new();
        let obj = oid(1);
        let b1 = bid(10);
        let b2 = bid(11);
        t.add_obj_ref(obj, b1).unwrap();
        t.add_obj_ref(obj, b2).unwrap();
        let refs = t.get_obj_refs(&obj);
        assert_eq!(refs.len(), 2);
    }

    #[test]
    fn test_no_duplicate_obj_refs() {
        let t = ReferenceTracker::new();
        let obj = oid(2);
        let b = bid(20);
        t.add_obj_ref(obj, b).unwrap();
        t.add_obj_ref(obj, b).unwrap(); // doublon
        let refs = t.get_obj_refs(&obj);
        assert_eq!(refs.len(), 1);
    }

    #[test]
    fn test_add_blob_ref() {
        let t = ReferenceTracker::new();
        let parent = bid(30);
        let child = bid(31);
        t.add_blob_ref(parent, child).unwrap();
        let subs = t.get_refs(&parent);
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0], child);
    }

    #[test]
    fn test_all_reachable_blobs_chain() {
        let t = ReferenceTracker::new();
        let obj = oid(3);
        let b1 = bid(40);
        let b2 = bid(41);
        let b3 = bid(42);
        t.add_obj_ref(obj, b1).unwrap();
        t.add_blob_ref(b1, b2).unwrap();
        t.add_blob_ref(b2, b3).unwrap();
        let all = t.all_reachable_blobs(&obj).unwrap();
        assert_eq!(all.len(), 3);
        assert!(all.contains(&b1));
        assert!(all.contains(&b2));
        assert!(all.contains(&b3));
    }

    #[test]
    fn test_all_reachable_blobs_cycle_safe() {
        // Cycle : b1 -> b2 -> b1 (ne doit pas boucler infiniment).
        let t = ReferenceTracker::new();
        let obj = oid(4);
        let b1 = bid(50);
        let b2 = bid(51);
        t.add_obj_ref(obj, b1).unwrap();
        t.add_blob_ref(b1, b2).unwrap();
        t.add_blob_ref(b2, b1).unwrap(); // cycle
        let all = t.all_reachable_blobs(&obj).unwrap();
        // Doit terminer et contenir les 2 blobs.
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_remove_obj() {
        let t = ReferenceTracker::new();
        let obj = oid(5);
        let b = bid(60);
        t.add_obj_ref(obj, b).unwrap();
        t.remove_obj(&obj);
        assert!(t.get_obj_refs(&obj).is_empty());
    }

    #[test]
    fn test_stats() {
        let t = ReferenceTracker::new();
        t.add_obj_ref(oid(6), bid(70)).unwrap();
        t.add_obj_ref(oid(6), bid(71)).unwrap();
        let s = t.stats();
        assert_eq!(s.obj_blob_refs, 2);
    }
}
