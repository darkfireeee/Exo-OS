// kernel/src/fs/exofs/gc/epoch_scanner.rs
//
// ==============================================================================
// Scanner des EpochRoots pour la phase de scan GC (GC-06)
// Ring 0 . no_std . Exo-OS
//
// Ce module lit les EpochRoots des slots A/B/C pour construire l'ensemble
// initial de racines GC (grey roots) avant la phase de marquage tricolore.
//
// Conformite :
//   GC-06 : les racines GC = EpochRoots des slots A/B/C valides
//   DEAD-01 : jamais d'acquisition de EPOCH_COMMIT_LOCK
//   RECUR-01 : tout traitement iteratif, pas recursif
//   OOM-02 : try_reserve avant chaque push
//   DAG-01 : pas d'import de ipc/, process/, arch/
// ==============================================================================

#![allow(dead_code)]

use alloc::collections::BTreeMap;
use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use core::fmt;

use crate::fs::exofs::core::{
    BlobId, DiskOffset, EpochId, ExofsError, ExofsResult, ObjectId,
};
use crate::fs::exofs::epoch::epoch_root::{EpochRootEntry, EpochRootInMemory};
use crate::fs::exofs::epoch::epoch_slots::EpochSlot;
use crate::fs::exofs::epoch::epoch_gc::GcEpochWindow;
use crate::fs::exofs::gc::tricolor::TricolorWorkspace;
use crate::scheduler::sync::spinlock::SpinLock;

// ==============================================================================
// Constantes
// ==============================================================================

/// Nombre maximal d'objets racines scannes par passe.
pub const MAX_SCAN_ROOTS: usize = 262_144;

/// Nombre maximal de BlobIds extraites d'une seule EpochRoot.
pub const MAX_BLOBS_PER_ROOT: usize = 65_536;

// ==============================================================================
// RootObject — un objet racine extrait d'une EpochRoot
// ==============================================================================

/// Un objet identifie depuis une EpochRoot scannee.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RootObject {
    /// Identifiant de l'objet.
    pub object_id:   ObjectId,
    /// Offset disque de l'objet (de l'EpochRootEntry).
    pub disk_offset: DiskOffset,
    /// Slot source (A, B ou C).
    pub slot:        EpochSlot,
    /// Epoch a laquelle l'objet a ete vu.
    pub epoch_id:    EpochId,
    /// L'objet est marque comme supprime.
    pub is_deleted:  bool,
}

// ==============================================================================
// SlotScanResult — resultat du scan d'un slot
// ==============================================================================

/// Resultat du scan d'un slot EpochRoot individuel.
#[derive(Debug, Default, Clone)]
pub struct SlotScanResult {
    pub slot:           EpochSlot,
    pub epoch_id:       EpochId,
    /// Objets modifies trouves.
    pub modified_count: usize,
    /// Objets supprimes trouves.
    pub deleted_count:  usize,
    /// Erreur rencontree durant le scan (si present le slot est ignore).
    pub scan_error:     Option<ExofsError>,
}

impl Default for EpochSlot {
    fn default() -> Self {
        EpochSlot::SlotA
    }
}

// ==============================================================================
// ScanStats — statistiques du scan complet
// ==============================================================================

/// Statistiques du scan de tous les slots GC-06.
#[derive(Debug, Default, Clone)]
pub struct ScanStats {
    /// Slots scannes.
    pub slots_scanned:       u64,
    /// Slots valides.
    pub slots_valid:         u64,
    /// Slots ignores (erreur ou vides).
    pub slots_skipped:       u64,
    /// Nombre total d'objets racines extraits.
    pub roots_extracted:     u64,
    /// Nombre d'objets non supprimes (candidats live).
    pub live_roots:          u64,
    /// Nombre d'objets marques supprimes.
    pub deleted_roots:       u64,
    /// BlobIds grises dans le workspace.
    pub blobs_greyed:        u64,
    /// Erreurs de file GcQueueFull.
    pub queue_full_errors:   u64,
}

impl fmt::Display for ScanStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ScanStats[slots={}/{} roots={} live={} del={} blobs_greyed={} qfull={}]",
            self.slots_valid,
            self.slots_scanned,
            self.roots_extracted,
            self.live_roots,
            self.deleted_roots,
            self.blobs_greyed,
            self.queue_full_errors,
        )
    }
}

// ==============================================================================
// EpochScanSnapshot — snapshot des racines extraites
// ==============================================================================

/// Resultat complet d'un scan des EpochRoots.
#[derive(Debug, Default, Clone)]
pub struct EpochScanSnapshot {
    /// Tous les objets racines extraits.
    pub root_objects: Vec<RootObject>,
    /// Ensemble des ObjectIds supprimes (a ne PAS griser).
    pub deleted_set:  BTreeSet<ObjectId>,
    /// Stats du scan.
    pub stats:        ScanStats,
}

impl EpochScanSnapshot {
    /// Construit un snapshot vide.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Objets vivants (non supprimes).
    pub fn live_objects(&self) -> impl Iterator<Item = &RootObject> {
        self.root_objects.iter().filter(|r| !r.is_deleted)
    }

    /// Est-ce qu'un objet est marque supprime dans ce scan ?
    pub fn is_deleted(&self, oid: &ObjectId) -> bool {
        self.deleted_set.contains(oid)
    }

    /// Nombre de racines vivantes.
    pub fn live_count(&self) -> usize {
        self.root_objects.iter().filter(|r| !r.is_deleted).count()
    }
}

// ==============================================================================
// BlobLookup — trait pour la resolution ObjectId -> BlobIds
// ==============================================================================

/// Trait permettant a l'EpochScanner de resoudre ObjectId en BlobIds.
///
/// Implementer ce trait pour brancher le scanner sur la table d'objets.
pub trait BlobLookup {
    fn blobs_for_object(&self, oid: &ObjectId) -> &[BlobId];
}

/// Implémentation vide pour les tests.
pub struct EmptyBlobLookup;

impl BlobLookup for EmptyBlobLookup {
    fn blobs_for_object(&self, _oid: &ObjectId) -> &[BlobId] {
        &[]
    }
}

// ==============================================================================
// EpochScannerInner — donnees proteges
// ==============================================================================

struct EpochScannerInner {
    /// Dernier snapshot de scan.
    last_snapshot: Option<EpochScanSnapshot>,
    /// Stats cumulees.
    total_stats:   ScanStats,
    /// Nombre de passes de scan.
    pass_count:    u64,
}

// ==============================================================================
// EpochScanner — facade thread-safe
// ==============================================================================

/// Scanner des EpochRoots pour la phase de scan GC.
pub struct EpochScanner {
    inner: SpinLock<EpochScannerInner>,
}

impl EpochScanner {
    pub const fn new() -> Self {
        Self {
            inner: SpinLock::new(EpochScannerInner {
                last_snapshot: None,
                total_stats:   ScanStats {
                    slots_scanned:     0,
                    slots_valid:       0,
                    slots_skipped:     0,
                    roots_extracted:   0,
                    live_roots:        0,
                    deleted_roots:     0,
                    blobs_greyed:      0,
                    queue_full_errors: 0,
                },
                pass_count: 0,
            }),
        }
    }

    // ── Scan principal (GC-06) ───────────────────────────────────────────────

    /// Scanne les EpochRoots fournis et extrait les objets racines GC.
    ///
    /// # Arguments
    /// - `epoch_roots` : une slice d'EpochRootInMemory pour chaque slot (A, B, C)
    ///   dans le meme ordre que `EpochSlot::all()`.
    ///   Si un slot n'est pas disponible, passer `None`.
    ///
    /// GC-06: racines = objets non supprimes de tous les slots valides.
    /// DEAD-01: cette methode n'acquiert JAMAIS EPOCH_COMMIT_LOCK.
    pub fn scan(
        &self,
        epoch_roots: &[Option<&EpochRootInMemory>],
    ) -> ExofsResult<EpochScanSnapshot> {
        let slots = EpochSlot::all(); // [SlotA, SlotB, SlotC]
        let mut snapshot = EpochScanSnapshot::empty();
        let mut stats = ScanStats::default();

        // RECUR-01 : iteration sur les slots, pas de recursion.
        for (i, slot) in slots.iter().enumerate() {
            stats.slots_scanned = stats.slots_scanned.saturating_add(1);

            let root = match epoch_roots.get(i).and_then(|r| *r) {
                Some(r) => r,
                None => {
                    stats.slots_skipped = stats.slots_skipped.saturating_add(1);
                    continue;
                }
            };

            let result = self.scan_single_root(root, *slot, &mut snapshot)?;
            stats.slots_valid = stats.slots_valid.saturating_add(1);
            stats.roots_extracted = stats.roots_extracted
                .saturating_add(result.modified_count as u64)
                .saturating_add(result.deleted_count as u64);
            stats.live_roots = stats.live_roots
                .saturating_add(result.modified_count as u64);
            stats.deleted_roots = stats.deleted_roots
                .saturating_add(result.deleted_count as u64);
        }

        snapshot.stats = stats.clone();

        // Mise a jour de l'etat interne.
        {
            let mut g = self.inner.lock();
            g.pass_count = g.pass_count.saturating_add(1);
            // Cumuler les stats totales.
            g.total_stats.slots_scanned = g.total_stats.slots_scanned
                .saturating_add(stats.slots_scanned);
            g.total_stats.roots_extracted = g.total_stats.roots_extracted
                .saturating_add(stats.roots_extracted);
            g.total_stats.live_roots = g.total_stats.live_roots
                .saturating_add(stats.live_roots);
            g.total_stats.deleted_roots = g.total_stats.deleted_roots
                .saturating_add(stats.deleted_roots);
            g.last_snapshot = Some(snapshot.clone());
        }

        Ok(snapshot)
    }

    /// Scanne un seul EpochRoot et ajoute les objets au snapshot.
    fn scan_single_root(
        &self,
        root:     &EpochRootInMemory,
        slot:     EpochSlot,
        snapshot: &mut EpochScanSnapshot,
    ) -> ExofsResult<SlotScanResult> {
        let mut result = SlotScanResult {
            slot,
            epoch_id:       root.epoch_id,
            modified_count: 0,
            deleted_count:  0,
            scan_error:     None,
        };

        // 1. Parcourir les objets modifies (vivants potentiels).
        for entry in &root.modified_objects {
            if snapshot.root_objects.len() >= MAX_SCAN_ROOTS {
                break;
            }

            let oid_bytes = entry.object_id;
            let oid = ObjectId(oid_bytes);

            let ro = RootObject {
                object_id:   oid,
                disk_offset: entry.disk_offset,
                slot,
                epoch_id:    root.epoch_id,
                is_deleted:  false,
            };

            snapshot.root_objects.try_reserve(1)
                .map_err(|_| ExofsError::NoMemory)?;
            snapshot.root_objects.push(ro);
            result.modified_count = result.modified_count.saturating_add(1);
        }

        // 2. Parcourir les objets supprimes.
        for oid in &root.deleted_objects {
            if snapshot.root_objects.len() >= MAX_SCAN_ROOTS {
                break;
            }

            let ro = RootObject {
                object_id:   *oid,
                disk_offset: DiskOffset::zero(),
                slot,
                epoch_id:    root.epoch_id,
                is_deleted:  true,
            };

            snapshot.root_objects.try_reserve(1)
                .map_err(|_| ExofsError::NoMemory)?;
            snapshot.root_objects.push(ro);

            snapshot.deleted_set.insert(*oid);
            result.deleted_count = result.deleted_count.saturating_add(1);
        }

        Ok(result)
    }

    // ── Construction du grey set (GC-06 → tricolor) ──────────────────────────

    /// A partir d'un snapshot de scan, grise tous les BlobIds associes aux
    /// objets vivants dans un TricolorWorkspace.
    ///
    /// Le `lookup` est utilise pour resoudre ObjectId -> BlobIds.
    ///
    /// Retourne le nombre de BlobIds grises.
    pub fn build_grey_set<L: BlobLookup>(
        &self,
        snapshot:  &EpochScanSnapshot,
        workspace: &mut TricolorWorkspace,
        lookup:    &L,
    ) -> ExofsResult<u64> {
        let mut greyed: BTreeSet<BlobId> = BTreeSet::new();
        let mut count: u64 = 0;
        let mut queue_full: u64 = 0;

        // RECUR-01 : boucle iterative.
        for ro in snapshot.live_objects() {
            if snapshot.is_deleted(&ro.object_id) {
                continue;
            }

            let blobs = lookup.blobs_for_object(&ro.object_id);
            for &blob_id in blobs {
                if greyed.contains(&blob_id) {
                    continue;
                }
                if greyed.len() >= MAX_BLOBS_PER_ROOT {
                    break;
                }
                greyed.insert(blob_id);

                match workspace.grey(blob_id) {
                    Ok(()) => {
                        count = count.saturating_add(1);
                    }
                    Err(ExofsError::GcQueueFull) => {
                        queue_full = queue_full.saturating_add(1);
                        // GC-03 : file pleine, on ne peut aller plus loin.
                        // La passe suivante s'occupera des noeuds restants.
                    }
                    Err(e) => return Err(e),
                }
            }
        }

        // Mise a jour des stats.
        {
            let mut g = self.inner.lock();
            g.total_stats.blobs_greyed = g.total_stats.blobs_greyed
                .saturating_add(count);
            g.total_stats.queue_full_errors = g.total_stats.queue_full_errors
                .saturating_add(queue_full);
        }

        Ok(count)
    }

    /// Version tout-en-un : scan + build grey set.
    ///
    /// Retourne le snapshot et les stats.
    pub fn scan_and_grey<L: BlobLookup>(
        &self,
        epoch_roots: &[Option<&EpochRootInMemory>],
        workspace:   &mut TricolorWorkspace,
        lookup:      &L,
    ) -> ExofsResult<EpochScanSnapshot> {
        let snapshot = self.scan(epoch_roots)?;
        let _greyed = self.build_grey_set(&snapshot, workspace, lookup)?;
        Ok(snapshot)
    }

    // ── Accesseurs ──────────────────────────────────────────────────────────

    /// Dernier snapshot de scan.
    pub fn last_snapshot(&self) -> Option<EpochScanSnapshot> {
        self.inner.lock().last_snapshot.clone()
    }

    /// Nombre de passes de scan.
    pub fn pass_count(&self) -> u64 {
        self.inner.lock().pass_count
    }

    /// Stats cumulees.
    pub fn total_stats(&self) -> ScanStats {
        self.inner.lock().total_stats.clone()
    }

    /// Reset du scanner (nouvelle passe GC).
    pub fn reset(&self) {
        let mut g = self.inner.lock();
        g.last_snapshot = None;
    }
}

// ==============================================================================
// Instance globale
// ==============================================================================

/// Scanner d'EpochRoots global pour le GC.
pub static EPOCH_SCANNER: EpochScanner = EpochScanner::new();

// ==============================================================================
// Tests
// ==============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::exofs::core::{BlobId, EpochFlags, ObjectId};
    use crate::fs::exofs::epoch::epoch_root::EpochRootInMemory;
    use crate::fs::exofs::gc::tricolor::{BlobNode, TricolorWorkspace};

    fn oid(b: u8) -> ObjectId {
        let mut a = [0u8; 32]; a[0] = b; ObjectId(a)
    }

    fn bid(b: u8) -> BlobId {
        let mut a = [0u8; 32]; a[0] = b; BlobId(a)
    }

    fn offset(n: u64) -> DiskOffset {
        DiskOffset::from_raw(n)
    }

    struct SimpleLookup {
        map: BTreeMap<ObjectId, Vec<BlobId>>,
    }

    impl BlobLookup for SimpleLookup {
        fn blobs_for_object(&self, oid: &ObjectId) -> &[BlobId] {
            self.map.get(oid).map(Vec::as_slice).unwrap_or(&[])
        }
    }

    #[test]
    fn test_scan_empty_roots() {
        let scanner = EpochScanner::new();
        let snap = scanner.scan(&[]).unwrap();
        assert_eq!(snap.root_objects.len(), 0);
        assert_eq!(snap.live_count(), 0);
    }

    #[test]
    fn test_scan_single_modified_object() {
        let scanner = EpochScanner::new();
        let mut root = EpochRootInMemory::new(42);
        root.add_modified(oid(1).0, offset(1024), EpochRootEntry::FLAG_MODIFIED).unwrap();

        let snap = scanner.scan(&[Some(&root), None, None]).unwrap();
        assert_eq!(snap.live_count(), 1);
        assert_eq!(snap.stats.slots_valid, 1);
    }

    #[test]
    fn test_scan_deleted_not_live() {
        let scanner = EpochScanner::new();
        let mut root = EpochRootInMemory::new(10);
        root.add_deleted(oid(5)).unwrap();

        let snap = scanner.scan(&[Some(&root), None, None]).unwrap();
        assert!(snap.is_deleted(&oid(5)));
        assert_eq!(snap.live_count(), 0);
        assert_eq!(snap.stats.deleted_roots, 1);
    }

    #[test]
    fn test_build_grey_set() {
        let scanner = EpochScanner::new();
        let mut root = EpochRootInMemory::new(5);
        root.add_modified(oid(1).0, offset(512), EpochRootEntry::FLAG_CREATED).unwrap();

        // Lookup: oid(1) -> [bid(10), bid(11)]
        let mut map = BTreeMap::new();
        map.insert(oid(1), alloc::vec![bid(10), bid(11)]);
        let lookup = SimpleLookup { map };

        let mut ws = TricolorWorkspace::new().unwrap();
        ws.insert_node(BlobNode::new(bid(10), 512, 1, 5, 0, false));
        ws.insert_node(BlobNode::new(bid(11), 256, 1, 5, 0, false));

        let snap = scanner.scan(&[Some(&root), None, None]).unwrap();
        let count = scanner.build_grey_set(&snap, &mut ws, &lookup).unwrap();

        assert_eq!(count, 2);
        assert_eq!(ws.grey_queue_len(), 2);
    }

    #[test]
    fn test_three_slots_merged() {
        let scanner = EpochScanner::new();

        let mut root_a = EpochRootInMemory::new(1);
        root_a.add_modified(oid(1).0, offset(0), EpochRootEntry::FLAG_CREATED).unwrap();

        let mut root_b = EpochRootInMemory::new(2);
        root_b.add_modified(oid(2).0, offset(0), EpochRootEntry::FLAG_MODIFIED).unwrap();

        let mut root_c = EpochRootInMemory::new(3);
        root_c.add_modified(oid(3).0, offset(0), EpochRootEntry::FLAG_MODIFIED).unwrap();

        let snap = scanner.scan(&[Some(&root_a), Some(&root_b), Some(&root_c)]).unwrap();
        assert_eq!(snap.stats.slots_valid, 3);
        assert_eq!(snap.live_count(), 3);
    }
}
