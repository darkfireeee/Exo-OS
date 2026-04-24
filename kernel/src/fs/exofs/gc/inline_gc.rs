// kernel/src/fs/exofs/gc/inline_gc.rs
//
// ==============================================================================
// GC pour les Objets Inline (données < 512 octets stockées dans l'ObjectTable)
// Ring 0 . no_std . Exo-OS
//
// Les LogicalObjects dont les données ne dépassent pas INLINE_DATA_THRESHOLD
// ne créent pas de Blob physique : leurs données sont stockées directement dans
// l'entrée de la table d'objets (inline storage).
//
// Ce module collecte les objets inline orphelins — ceux dont le ref_count est 0
// et qui ne sont plus référencés par aucun EpochRoot valide.
//
// Conformite :
//   GC-07 : jamais collecter un objet EPOCH_PINNED
//   RECUR-01 : traversee iterative
//   OOM-02 : try_reserve avant push
//   ARITH-02 : saturating_*
//   DAG-01 : pas d'import de ipc/, process/, arch/
// ==============================================================================

use alloc::collections::BTreeMap;
use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use core::fmt;

use crate::fs::exofs::core::{EpochId, ExofsError, ExofsResult, ObjectId};
use crate::fs::exofs::epoch::epoch_pin::is_epoch_pinned;
use crate::fs::exofs::gc::epoch_scanner::EpochScanSnapshot;
use crate::fs::exofs::gc::gc_metrics::GC_METRICS;
use crate::fs::exofs::gc::gc_state::GC_STATE;
use crate::scheduler::sync::spinlock::SpinLock;

// ==============================================================================
// Constantes
// ==============================================================================

/// Taille maximale des données inline (octets).
pub const INLINE_DATA_THRESHOLD: usize = 512;

/// Nombre maximum d'objets inline traites par passe.
pub const MAX_INLINE_PER_PASS: usize = 65536;

/// Taille de batch pour le traitement inline.
pub const INLINE_BATCH_SIZE: usize = 256;

// ==============================================================================
// InlineObjectEntry — entrée d'un objet inline dans le registre
// ==============================================================================

/// Entrée représentant un objet avec stockage inline.
#[derive(Debug, Clone)]
pub struct InlineObjectEntry {
    /// Identifiant de l'objet.
    pub object_id: ObjectId,
    /// Taille des données inline (en octets, <= INLINE_DATA_THRESHOLD).
    pub data_size: u32,
    /// Epoch de création.
    pub create_epoch: EpochId,
    /// Ref_count atomique de cet objet inline.
    ref_count: u32, // Simplifié : u32 (pas atomique dans la struct).
}

impl InlineObjectEntry {
    /// Crée une nouvelle entrée inline.
    pub fn new(object_id: ObjectId, data_size: u32, create_epoch: EpochId) -> Self {
        assert!(
            data_size as usize <= INLINE_DATA_THRESHOLD,
            "InlineObjectEntry: data_size {} > INLINE_DATA_THRESHOLD {}",
            data_size,
            INLINE_DATA_THRESHOLD,
        );
        Self {
            object_id,
            data_size,
            create_epoch,
            ref_count: 1,
        }
    }

    /// Ref_count courant.
    pub fn ref_count(&self) -> u32 {
        self.ref_count
    }

    /// Est-ce que cet objet est collectible (ref_count = 0) ?
    pub fn is_collectible(&self) -> bool {
        self.ref_count == 0
    }
}

// ==============================================================================
// InlineGcStats — statistiques
// ==============================================================================

/// Statistiques du GC inline.
#[derive(Debug, Default, Clone)]
pub struct InlineGcStats {
    /// Objets inline enregistres.
    pub registered: u64,
    /// Objets inline supprimes.
    pub collected: u64,
    /// Octets liberes.
    pub bytes_freed: u64,
    /// Objets sautes car EPOCH_PINNED (GC-07).
    pub pinned_skipped: u64,
    /// Objets sautes car encore references.
    pub refcount_skipped: u64,
    /// Passes effectuees.
    pub passes: u64,
    /// Nombre courant d'objets inline.
    pub current_count: u64,
}

impl fmt::Display for InlineGcStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "InlineGcStats[reg={} coll={} bytes={} pinned={} rc_skip={} passes={}]",
            self.registered,
            self.collected,
            self.bytes_freed,
            self.pinned_skipped,
            self.refcount_skipped,
            self.passes,
        )
    }
}

// ==============================================================================
// InlineGcResult — résultat d'une passe
// ==============================================================================

/// Résultat d'une passe du GC inline.
#[derive(Debug, Default, Clone)]
pub struct InlineGcResult {
    /// Objets analyses.
    pub analyzed: u64,
    /// Objets collectes (ref_count = 0 + non pinned).
    pub collected: u64,
    /// Octets liberes.
    pub bytes_freed: u64,
    /// Objets sautes EPOCH_PINNED.
    pub pinned_skipped: u64,
    /// Objets sautes car ref_count > 0.
    pub refcount_skipped: u64,
    /// Phase complete.
    pub phase_complete: bool,
}

impl fmt::Display for InlineGcResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "InlineGcResult[analyzed={} collected={} bytes={} pinned={}]",
            self.analyzed, self.collected, self.bytes_freed, self.pinned_skipped,
        )
    }
}

// ==============================================================================
// InlineGcInner — état interne
// ==============================================================================

struct InlineGcInner {
    /// Registre des objets inline connus.
    registry: BTreeMap<ObjectId, InlineObjectEntry>,
    /// Stats cumulees.
    stats: InlineGcStats,
}

// ==============================================================================
// InlineGc — facade thread-safe
// ==============================================================================

/// Gestionnaire GC pour les objets inline.
pub struct InlineGc {
    inner: SpinLock<InlineGcInner>,
}

impl InlineGc {
    pub const fn new() -> Self {
        Self {
            inner: SpinLock::new(InlineGcInner {
                registry: BTreeMap::new(),
                stats: InlineGcStats {
                    registered: 0,
                    collected: 0,
                    bytes_freed: 0,
                    pinned_skipped: 0,
                    refcount_skipped: 0,
                    passes: 0,
                    current_count: 0,
                },
            }),
        }
    }

    // ── Gestion du registre ──────────────────────────────────────────────────

    /// Enregistre un objet inline.
    ///
    /// OOM-02 : try_reserve implicite via BTreeMap (pas de try_reserve disponible,
    /// mais on borne la taille du registre).
    pub fn register(&self, entry: InlineObjectEntry) -> ExofsResult<()> {
        let mut g = self.inner.lock();

        if g.registry.len() >= MAX_INLINE_PER_PASS {
            return Err(ExofsError::Resource);
        }

        let oid = entry.object_id;
        g.registry.insert(oid, entry);
        g.stats.registered = g.stats.registered.saturating_add(1);
        g.stats.current_count = g.registry.len() as u64;

        Ok(())
    }

    /// Incrémente le ref_count d'un objet inline.
    pub fn inc_ref(&self, oid: &ObjectId) -> ExofsResult<()> {
        let mut g = self.inner.lock();
        match g.registry.get_mut(oid) {
            Some(entry) => {
                entry.ref_count = entry
                    .ref_count
                    .checked_add(1)
                    .ok_or(ExofsError::InternalError)?;
                Ok(())
            }
            None => Err(ExofsError::BlobNotFound),
        }
    }

    /// Décrémente le ref_count d'un objet inline (REFCNT-01 : panic sur underflow).
    pub fn dec_ref(&self, oid: &ObjectId) -> ExofsResult<bool> {
        let mut g = self.inner.lock();
        match g.registry.get_mut(oid) {
            Some(entry) => {
                if entry.ref_count == 0 {
                    // REFCNT-01 : underflow interdit.
                    panic!("InlineGc::dec_ref: refcount underflow for {:?}", oid);
                }
                entry.ref_count -= 1;
                Ok(entry.ref_count == 0)
            }
            None => Err(ExofsError::BlobNotFound),
        }
    }

    /// Retire un objet inline du registre.
    pub fn remove(&self, oid: &ObjectId) {
        let mut g = self.inner.lock();
        g.registry.remove(oid);
        g.stats.current_count = g.registry.len() as u64;
    }

    // ── Phase de collecte ────────────────────────────────────────────────────

    /// Lance une passe de GC sur les objets inline.
    ///
    /// Collecte tous les objets inline avec ref_count = 0 qui ne sont plus
    /// references dans le snapshot de scan des EpochRoots.
    ///
    /// GC-07 : sauter les objets dont l'epoch est pinnee.
    pub fn collect(&self, scan_snapshot: &EpochScanSnapshot) -> ExofsResult<InlineGcResult> {
        // Ensemble des ObjectIds vivants (depuis les EpochRoots).
        let reachable: BTreeSet<ObjectId> = scan_snapshot
            .live_objects()
            .map(|ro| ro.object_id)
            .collect();

        let to_collect: Vec<(ObjectId, u32, EpochId)> = {
            let g = self.inner.lock();
            g.registry
                .iter()
                .filter(|(oid, entry)| !reachable.contains(*oid) && entry.is_collectible())
                .map(|(oid, entry)| (*oid, entry.data_size, entry.create_epoch))
                .take(MAX_INLINE_PER_PASS)
                .collect()
        };

        let mut result = InlineGcResult::default();
        result.analyzed = to_collect.len() as u64;

        // RECUR-01 : parcours iteratif.
        for (oid, data_size, create_epoch) in to_collect {
            // Verifier le ref_count en live.
            let rc = {
                let g = self.inner.lock();
                g.registry.get(&oid).map(|e| e.ref_count).unwrap_or(1)
            };

            if rc > 0 {
                result.refcount_skipped = result.refcount_skipped.saturating_add(1);
                continue;
            }

            // GC-07 : verifier si l'epoch est pinnee.
            if is_epoch_pinned(create_epoch) {
                result.pinned_skipped = result.pinned_skipped.saturating_add(1);
                continue;
            }

            // Supprimer l'objet inline.
            {
                let mut g = self.inner.lock();
                g.registry.remove(&oid);
                g.stats.collected = g.stats.collected.saturating_add(1);
                g.stats.bytes_freed = g.stats.bytes_freed.saturating_add(data_size as u64);
                g.stats.current_count = g.registry.len() as u64;
            }

            result.collected = result.collected.saturating_add(1);
            result.bytes_freed = result.bytes_freed.saturating_add(data_size as u64);
        }

        result.phase_complete = true;

        // Metriques globales.
        GC_METRICS.add_blobs_collected(result.collected);
        GC_METRICS.add_bytes_freed(result.bytes_freed);
        GC_STATE.record_inline_gc(result.collected);

        // Stats internes.
        {
            let mut g = self.inner.lock();
            g.stats.passes = g.stats.passes.saturating_add(1);
            g.stats.pinned_skipped = g.stats.pinned_skipped.saturating_add(result.pinned_skipped);
            g.stats.refcount_skipped = g
                .stats
                .refcount_skipped
                .saturating_add(result.refcount_skipped);
        }

        Ok(result)
    }

    // ── Accesseurs ──────────────────────────────────────────────────────────

    /// Nombre d'objets inline connus.
    pub fn count(&self) -> usize {
        self.inner.lock().registry.len()
    }

    /// Stats cumulees.
    pub fn stats(&self) -> InlineGcStats {
        let g = self.inner.lock();
        let mut s = g.stats.clone();
        s.current_count = g.registry.len() as u64;
        s
    }

    /// Ref_count courant d'un objet.
    pub fn ref_count_of(&self, oid: &ObjectId) -> Option<u32> {
        self.inner.lock().registry.get(oid).map(|e| e.ref_count)
    }

    /// Reset complet (nouvelle passe GC).
    pub fn clear(&self) {
        let mut g = self.inner.lock();
        g.registry.clear();
        g.stats.current_count = 0;
    }
}

// ==============================================================================
// Instance globale
// ==============================================================================

/// GC inline global.
pub static INLINE_GC: InlineGc = InlineGc::new();

// ==============================================================================
// Tests
// ==============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::exofs::core::ObjectId;
    use crate::fs::exofs::gc::epoch_scanner::EpochScanSnapshot;

    fn oid(b: u8) -> ObjectId {
        let mut a = [0u8; 32];
        a[0] = b;
        ObjectId(a)
    }

    fn entry(b: u8, size: u32) -> InlineObjectEntry {
        let mut e = InlineObjectEntry::new(oid(b), size, EpochId(0));
        e.ref_count = 0; // Orphelin dès le départ pour les tests.
        e
    }

    #[test]
    fn test_register_and_count() {
        let gc = InlineGc::new();
        let e = InlineObjectEntry::new(oid(1), 64, EpochId(0));
        gc.register(e).unwrap();
        assert_eq!(gc.count(), 1);
    }

    #[test]
    fn test_collect_orphan() {
        let gc = InlineGc::new();
        // Objet inline avec ref_count = 0.
        gc.register(entry(1, 128)).unwrap();

        let snap = EpochScanSnapshot::empty();
        let result = gc.collect(&snap).unwrap();
        // ref_count=0 et non atteignable -> collecte.
        assert_eq!(result.collected, 1);
        assert_eq!(result.bytes_freed, 128);
        assert_eq!(gc.count(), 0);
    }

    #[test]
    fn test_collect_skips_reachable() {
        let gc = InlineGc::new();
        let mut e = InlineObjectEntry::new(oid(2), 64, EpochId(0));
        e.ref_count = 0;
        gc.register(e).unwrap();

        // L'objet oid(2) est dans le scan snapshot (reachable).
        let mut snap = EpochScanSnapshot::empty();
        use crate::fs::exofs::core::DiskOffset;
        use crate::fs::exofs::epoch::epoch_slots::EpochSlot;
        use crate::fs::exofs::gc::epoch_scanner::RootObject;
        snap.root_objects.push(RootObject {
            object_id: oid(2),
            disk_offset: DiskOffset::zero(),
            slot: EpochSlot::A,
            epoch_id: EpochId(1),
            is_deleted: false,
        });

        let result = gc.collect(&snap).unwrap();
        // Reachable => saute.
        assert_eq!(result.collected, 0);
        assert_eq!(gc.count(), 1);
    }

    #[test]
    fn test_inc_dec_ref() {
        let gc = InlineGc::new();
        gc.register(InlineObjectEntry::new(oid(3), 32, EpochId(0)))
            .unwrap();
        assert_eq!(gc.ref_count_of(&oid(3)), Some(1));

        gc.inc_ref(&oid(3)).unwrap();
        assert_eq!(gc.ref_count_of(&oid(3)), Some(2));

        gc.dec_ref(&oid(3)).unwrap();
        assert_eq!(gc.ref_count_of(&oid(3)), Some(1));
    }

    #[test]
    fn test_data_size_threshold() {
        // size = 512 est OK.
        let e = InlineObjectEntry::new(oid(5), 512, EpochId(0));
        assert_eq!(e.data_size, 512);
    }
}
