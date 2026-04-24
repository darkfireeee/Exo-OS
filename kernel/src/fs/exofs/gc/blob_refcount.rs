// kernel/src/fs/exofs/gc/blob_refcount.rs
//
// ==============================================================================
// Table de comptage de references pour les P-Blobs
// Ring 0 . no_std . Exo-OS
//
// Chaque P-Blob (Physical Blob) possede un ref_count u32 atomique.
// Cette table centralise tous ces compteurs pour le GC.
//
// Conformite :
//   REFCNT-01 : compare_exchange loop, PANIC sur underflow
//   GC-01     : DeferredDeleteQueue avec delai minimum 2 Epochs
//   GC-08     : Creation atomique — alloc + ref_count.store(1) + insert indivisible
//   GC-09     : INTERDIT creer P-Blob sans ref_count=1 immediat
//   RACE-01   : store(ref_count=1) -> barrier -> insert(table)
//   OOM-02    : try_reserve avant chaque insertion
//   ARITH-02  : checked_add / saturating_* partout
// ==============================================================================

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::fs::exofs::core::{BlobId, EpochId, ExofsError, ExofsResult};
use crate::scheduler::sync::spinlock::SpinLock;

// ==============================================================================
// Constantes
// ==============================================================================

/// Delai minimum en nombre d'epochs avant suppression effective (GC-01).
pub const GC_MIN_DEFERRED_EPOCHS: u64 = 2;

/// Capacite initiale de la DeferredDeleteQueue.
pub const DEFERRED_QUEUE_INITIAL_CAP: usize = 256;

/// Nombre maximum d'entrees dans la DeferredDeleteQueue.
pub const DEFERRED_QUEUE_MAX: usize = 4096;

// ==============================================================================
// RefEntry — entree par P-Blob
// ==============================================================================

/// Entree de comptage pour un P-Blob.
///
/// Separe les donnees atomiques des meta-donnees protegees par verrou.
struct RefEntry {
    /// Compteur de references live (REFCNT-01 : atomique + check).
    count: AtomicU32,
    /// Taille physique du blob en octets.
    phys_size: u64,
    /// Epoch de creation du blob.
    create_epoch: u64,
}

impl RefEntry {
    /// Cree une entree avec ref_count initial (GC-08 : toujours >= 1).
    fn new(initial: u32, phys_size: u64, create_epoch: u64) -> Self {
        debug_assert!(
            initial >= 1,
            "GC-09: ref_count doit etre >= 1 a la creation"
        );
        Self {
            count: AtomicU32::new(initial),
            phys_size,
            create_epoch,
        }
    }

    #[allow(dead_code)]
    fn load_count(&self) -> u32 {
        self.count.load(Ordering::Acquire)
    }
}

// ==============================================================================
// DeferredDeleteEntry — element de la queue de suppression differee
// ==============================================================================

/// Entree dans la file de suppression differee (GC-01).
#[derive(Debug, Clone)]
pub struct DeferredDeleteEntry {
    /// Blob a supprimer.
    pub blob_id: BlobId,
    /// Taille physique pour les metriques.
    pub phys_size: u64,
    /// Epoch a partir de laquelle la suppression peut s'effectuer.
    /// = epoch_queued + GC_MIN_DEFERRED_EPOCHS.
    pub min_epoch: u64,
}

// ==============================================================================
// RefcountStats — metriques de la table
// ==============================================================================

/// Statistiques de la table de references.
#[derive(Debug, Default, Clone)]
pub struct RefcountStats {
    /// Nombre de blobs enregistres.
    pub blobs_registered: u64,
    /// Nombre de blobs deregistres.
    pub blobs_removed: u64,
    /// Nombre total d'increments.
    pub inc_total: u64,
    /// Nombre total de decrements.
    pub dec_total: u64,
    /// Nombre de blobs passes a 0 (candidats GC).
    pub zeroed_count: u64,
    /// Entrees dans la DeferredDeleteQueue.
    pub deferred_queued: u64,
    /// Entrees effectivement supprimees depuis la DeferredDeleteQueue.
    pub deferred_flushed: u64,
}

impl fmt::Display for RefcountStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "RefcountStats[registered={} removed={} inc={} dec={} zeroed={} \
             deferred_q={} deferred_f={}]",
            self.blobs_registered,
            self.blobs_removed,
            self.inc_total,
            self.dec_total,
            self.zeroed_count,
            self.deferred_queued,
            self.deferred_flushed,
        )
    }
}

// ==============================================================================
// BlobRefcountInner — donnees protegees par SpinLock
// ==============================================================================

struct BlobRefcountInner {
    /// Table principale BlobId -> RefEntry.
    map: BTreeMap<BlobId, RefEntry>,
    /// File de suppression differee (GC-01).
    deferred: Vec<DeferredDeleteEntry>,
    /// Statistiques.
    stats: RefcountStats,
}

impl BlobRefcountInner {
    const fn new() -> Self {
        Self {
            map: BTreeMap::new(),
            deferred: Vec::new(),
            stats: RefcountStats {
                blobs_registered: 0,
                blobs_removed: 0,
                inc_total: 0,
                dec_total: 0,
                zeroed_count: 0,
                deferred_queued: 0,
                deferred_flushed: 0,
            },
        }
    }
}

// ==============================================================================
// BlobRefcount — facade thread-safe
// ==============================================================================

/// Table globale des compteurs de references P-Blob.
///
/// Thread-safe via SpinLock sur les mutations structurelles.
/// Les operations atomiques sur count n'ont pas besoin du verrou
/// mais le verrou est pris pour garantir la coherence map/deferred.
pub struct BlobRefcount {
    inner: SpinLock<BlobRefcountInner>,
    /// Compteur global d'octets de ref_count sum (pour metriques rapides).
    total_phys_bytes: AtomicU64,
}

impl BlobRefcount {
    pub const fn new() -> Self {
        Self {
            inner: SpinLock::new(BlobRefcountInner::new()),
            total_phys_bytes: AtomicU64::new(0),
        }
    }

    // ── Enregistrement ───────────────────────────────────────────────────────

    /// Enregistre un nouveau P-Blob.
    ///
    /// GC-08 : `initial` DOIT etre 1.
    /// GC-09 : panicke si initial == 0.
    /// RACE-01 : l'appelant doit appeler cette methode IMMEDIATEMENT apres
    ///           avoir alloue le blob, avant toute autre utilisation.
    pub fn register(&self, id: BlobId, phys_size: u64, create_epoch: EpochId) -> ExofsResult<()> {
        let mut g = self.inner.lock();

        if g.map.contains_key(&id) {
            return Err(ExofsError::ObjectAlreadyExists);
        }

        // OOM-02 : BTreeMap n'a pas de try_reserve standard ; on alloue
        // une entree avec valeur initiale 1 (GC-08/09).
        let entry = RefEntry::new(1, phys_size, create_epoch.0);

        // NOTE : try_reserve non disponible sur BTreeMap en no_std.
        // L'allocateur kernel panique sur OOM — comportement voulu Ring 0.
        g.map.insert(id, entry);

        // Mise a jour atomique des bytes physiques.
        self.total_phys_bytes
            .fetch_add(phys_size, Ordering::Relaxed);

        g.stats.blobs_registered = g.stats.blobs_registered.saturating_add(1);
        Ok(())
    }

    // ── Increment ────────────────────────────────────────────────────────────

    /// Incremente le ref_count d'un blob.
    ///
    /// ARITH-02 : checked_add — retourne Err(Overflow) si saturation.
    /// Retourne le nouveau compteur.
    pub fn inc(&self, id: &BlobId) -> ExofsResult<u32> {
        let g = self.inner.lock();
        match g.map.get(id) {
            None => Err(ExofsError::BlobNotFound),
            Some(entry) => {
                // Boucle compare_exchange (REFCNT-01).
                loop {
                    let current = entry.count.load(Ordering::Acquire);
                    let next = current.checked_add(1).ok_or(ExofsError::InternalError)?;
                    match entry.count.compare_exchange_weak(
                        current,
                        next,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    ) {
                        Ok(_) => {
                            drop(g);
                            // Mise a jour stats sans tenir le verrou interne.
                            // (On reprend le verrou pour stats — acceptable car rare.)
                            let mut g2 = self.inner.lock();
                            g2.stats.inc_total = g2.stats.inc_total.saturating_add(1);
                            return Ok(next);
                        }
                        Err(_) => continue,
                    }
                }
            }
        }
    }

    // ── Decrement ────────────────────────────────────────────────────────────

    /// Decremente le ref_count d'un blob.
    ///
    /// REFCNT-01 : retourne `RefCountUnderflow` si underflow (current == 0).
    /// Retourne `(nouveau_count, phys_size)`.
    /// Si nouveau_count == 0 : le blob est candidat a la DeferredDeleteQueue.
    pub fn dec(&self, id: &BlobId, current_epoch: EpochId) -> ExofsResult<(u32, u64)> {
        let mut g = self.inner.lock();
        match g.map.get(id) {
            None => Err(ExofsError::BlobNotFound),
            Some(entry) => {
                // Boucle compare_exchange (REFCNT-01).
                loop {
                    let current = entry.count.load(Ordering::Acquire);
                    if current == 0 {
                        return Err(ExofsError::RefCountUnderflow);
                    }
                    let next = current - 1; // Safe : current > 0
                    match entry.count.compare_exchange_weak(
                        current,
                        next,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    ) {
                        Ok(_) => {
                            let phys_size = entry.phys_size;
                            g.stats.dec_total = g.stats.dec_total.saturating_add(1);

                            if next == 0 {
                                // Candidat GC — ajouter a la DeferredDeleteQueue (GC-01).
                                g.stats.zeroed_count = g.stats.zeroed_count.saturating_add(1);
                                let min_epoch =
                                    current_epoch.0.saturating_add(GC_MIN_DEFERRED_EPOCHS);
                                // Verifier que la queue n'est pas pleine.
                                if g.deferred.len() < DEFERRED_QUEUE_MAX {
                                    // OOM-02.
                                    if g.deferred.try_reserve(1).is_ok() {
                                        g.deferred.push(DeferredDeleteEntry {
                                            blob_id: *id,
                                            phys_size,
                                            min_epoch,
                                        });
                                        g.stats.deferred_queued =
                                            g.stats.deferred_queued.saturating_add(1);
                                    }
                                }
                            }
                            return Ok((next, phys_size));
                        }
                        Err(_) => continue,
                    }
                }
            }
        }
    }

    /// Ajoute explicitement un blob deja a ref_count 0 dans la file differee.
    ///
    /// Utilise par les passes de GC qui tombent sur un blob deja orphelin.
    pub fn queue_zero(&self, id: &BlobId, current_epoch: EpochId) -> ExofsResult<u64> {
        let mut g = self.inner.lock();
        match g.map.get(id) {
            None => Err(ExofsError::BlobNotFound),
            Some(entry) => {
                let current = entry.count.load(Ordering::Acquire);
                let phys_size = entry.phys_size;
                if current != 0 {
                    return Err(ExofsError::InvalidState);
                }
                if g.deferred.iter().any(|queued| queued.blob_id == *id) {
                    return Ok(phys_size);
                }
                if g.deferred.len() >= DEFERRED_QUEUE_MAX {
                    return Err(ExofsError::Resource);
                }
                g.deferred.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                g.deferred.push(DeferredDeleteEntry {
                    blob_id: *id,
                    phys_size,
                    min_epoch: current_epoch.0.saturating_add(GC_MIN_DEFERRED_EPOCHS),
                });
                g.stats.deferred_queued = g.stats.deferred_queued.saturating_add(1);
                Ok(phys_size)
            }
        }
    }

    // ── Lecture ──────────────────────────────────────────────────────────────

    /// Retourne le ref_count courant (None si inconnu).
    pub fn get(&self, id: &BlobId) -> Option<u32> {
        let g = self.inner.lock();
        g.map.get(id).map(|e| e.count.load(Ordering::Acquire))
    }

    /// Retourne le ref_count courant, ou 0 si le blob est inconnu.
    pub fn get_count(&self, id: &BlobId) -> u32 {
        self.get(id).unwrap_or(0)
    }

    /// Retourne la taille physique d'un blob (None si inconnu).
    pub fn phys_size(&self, id: &BlobId) -> Option<u64> {
        let g = self.inner.lock();
        g.map.get(id).map(|e| e.phys_size)
    }

    /// Retourne l'epoch de création et la taille physique du blob.
    /// Retourne (EpochId::INVALID, 0) si le blob est inconnu.
    pub fn get_epoch_and_size(&self, id: &BlobId) -> (EpochId, u64) {
        let g = self.inner.lock();
        g.map
            .get(id)
            .map(|e| (EpochId(e.create_epoch), e.phys_size))
            .unwrap_or((EpochId::INVALID, 0))
    }

    /// Nombre total de blobs suivis.
    pub fn blob_count(&self) -> usize {
        self.inner.lock().map.len()
    }

    /// Total des octets physiques des blobs suivis.
    pub fn total_phys_bytes(&self) -> u64 {
        self.total_phys_bytes.load(Ordering::Relaxed)
    }

    // ── Suppression ──────────────────────────────────────────────────────────

    /// Supprime un blob dont le ref_count est 0.
    ///
    /// Retourne `ExofsError::Logic` si ref_count != 0.
    pub fn remove_zero(&self, id: &BlobId) -> ExofsResult<u64> {
        let mut g = self.inner.lock();
        match g.map.get(id) {
            None => Err(ExofsError::BlobNotFound),
            Some(e) if e.count.load(Ordering::Acquire) != 0 => Err(ExofsError::Logic),
            Some(e) => {
                let phys = e.phys_size;
                g.map.remove(id);
                self.total_phys_bytes.fetch_sub(phys, Ordering::Relaxed);
                g.stats.blobs_removed = g.stats.blobs_removed.saturating_add(1);
                Ok(phys)
            }
        }
    }

    // ── DeferredDeleteQueue ───────────────────────────────────────────────────

    /// Extrait les entrees de la DeferredDeleteQueue dont le delai est echu.
    ///
    /// GC-01 : seules les entrees avec min_epoch <= current_epoch sont extraites.
    /// Retourne la liste des blobs a supprimer.
    pub fn flush_deferred(&self, current_epoch: EpochId) -> Vec<DeferredDeleteEntry> {
        let mut g = self.inner.lock();
        let current = current_epoch.0;

        // Partition : ready = min_epoch <= current, reste = les autres.
        let mut ready = Vec::new();
        let mut remain = Vec::new();

        for entry in g.deferred.drain(..) {
            if entry.min_epoch <= current {
                ready.push(entry);
            } else {
                remain.push(entry);
            }
        }

        // OOM-02 : try_reserve pour le remain temporaire.
        if g.deferred.try_reserve_exact(remain.len()).is_ok() {
            g.deferred.extend(remain);
        }
        // Sinon : on perd les entrees non-matures — elles seront re-queues au prochain dec().

        g.stats.deferred_flushed = g.stats.deferred_flushed.saturating_add(ready.len() as u64);
        ready
    }

    /// Longueur courante de la DeferredDeleteQueue.
    pub fn deferred_len(&self) -> usize {
        self.inner.lock().deferred.len()
    }

    // ── Iteration pour le GC ─────────────────────────────────────────────────

    /// Collecte tous les blobs avec ref_count == 0 (candidats immediats).
    ///
    /// Utilisé par le sweeper pour identifier les blobs a supprimer.
    /// Ne modifie pas la table.
    pub fn collect_zero_refs(&self) -> Vec<(BlobId, u64)> {
        let g = self.inner.lock();
        let mut result = Vec::new();
        for (id, entry) in g.map.iter() {
            if entry.count.load(Ordering::Acquire) == 0 {
                result.push((*id, entry.phys_size));
            }
        }
        result
    }

    /// Retourne un snapshot des statistiques.
    pub fn stats(&self) -> RefcountStats {
        self.inner.lock().stats.clone()
    }
}

// ==============================================================================
// Instance globale
// ==============================================================================

/// Table de comptage de references globale pour tous les P-Blobs.
pub static BLOB_REFCOUNT: BlobRefcount = BlobRefcount::new();

// ==============================================================================
// Tests
// ==============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_blob_id(b: u8) -> BlobId {
        let mut arr = [0u8; 32];
        arr[0] = b;
        BlobId(arr)
    }

    fn make_blob_id_u16(v: u16) -> BlobId {
        let mut arr = [0u8; 32];
        arr[0] = (v & 0x00FF) as u8;
        arr[1] = (v >> 8) as u8;
        BlobId(arr)
    }

    fn epoch(v: u64) -> EpochId {
        EpochId(v)
    }

    #[test]
    fn test_register_and_get() {
        let table = BlobRefcount::new();
        let id = make_blob_id(1);
        table.register(id, 4096, epoch(1)).unwrap();
        assert_eq!(table.get(&id), Some(1));
        assert_eq!(table.phys_size(&id), Some(4096));
        assert_eq!(table.blob_count(), 1);
    }

    #[test]
    fn test_register_duplicate_fails() {
        let table = BlobRefcount::new();
        let id = make_blob_id(2);
        table.register(id, 1024, epoch(1)).unwrap();
        let r = table.register(id, 1024, epoch(1));
        assert!(r.is_err());
    }

    #[test]
    fn test_inc_dec() {
        let table = BlobRefcount::new();
        let id = make_blob_id(3);
        table.register(id, 512, epoch(1)).unwrap();
        assert_eq!(table.inc(&id).unwrap(), 2);
        assert_eq!(table.inc(&id).unwrap(), 3);
        let (new_count, _) = table.dec(&id, epoch(10)).unwrap();
        assert_eq!(new_count, 2);
    }

    #[test]
    fn test_dec_to_zero_adds_deferred() {
        let table = BlobRefcount::new();
        let id = make_blob_id(4);
        table.register(id, 2048, epoch(5)).unwrap();
        let (new_count, _) = table.dec(&id, epoch(10)).unwrap();
        assert_eq!(new_count, 0);
        assert_eq!(table.deferred_len(), 1);
    }

    #[test]
    fn test_flush_deferred_respects_min_epoch() {
        let table = BlobRefcount::new();
        let id = make_blob_id(5);
        table.register(id, 1024, epoch(1)).unwrap();
        table.dec(&id, epoch(3)).unwrap(); // min_epoch = 3 + 2 = 5
                                           // Epoch 4 : pas encore pret.
        let ready = table.flush_deferred(epoch(4));
        assert!(ready.is_empty());
        // Epoch 5 : pret.
        let ready = table.flush_deferred(epoch(5));
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].blob_id, id);
    }

    #[test]
    fn test_queue_zero_requeues_ready_blob_without_underflow() {
        let table = BlobRefcount::new();
        let id = make_blob_id(42);
        table.register(id, 1024, epoch(1)).unwrap();
        table.dec(&id, epoch(3)).unwrap();
        let ready = table.flush_deferred(epoch(5));
        assert_eq!(ready.len(), 1);
        assert_eq!(table.queue_zero(&id, epoch(6)).unwrap(), 1024);
        assert_eq!(table.deferred_len(), 1);
    }

    #[test]
    fn test_queue_zero_stress_many_zero_blobs() {
        let table = BlobRefcount::new();
        const COUNT: u16 = 192;

        for i in 0..COUNT {
            let id = make_blob_id_u16(i);
            table.register(id, 4096, epoch(1)).unwrap();
            table.dec(&id, epoch(4)).unwrap();
        }

        let ready = table.flush_deferred(epoch(6));
        assert_eq!(ready.len(), COUNT as usize);

        for i in 0..COUNT {
            let id = make_blob_id_u16(i);
            assert_eq!(table.queue_zero(&id, epoch(7)).unwrap(), 4096);
        }

        assert_eq!(table.deferred_len(), COUNT as usize);
    }

    #[test]
    fn test_remove_zero() {
        let table = BlobRefcount::new();
        let id = make_blob_id(6);
        table.register(id, 8192, epoch(1)).unwrap();
        table.dec(&id, epoch(10)).unwrap();
        let freed = table.remove_zero(&id).unwrap();
        assert_eq!(freed, 8192);
        assert_eq!(table.get(&id), None);
    }

    #[test]
    fn test_remove_nonzero_fails() {
        let table = BlobRefcount::new();
        let id = make_blob_id(7);
        table.register(id, 1024, epoch(1)).unwrap();
        // ref_count = 1, pas 0
        let r = table.remove_zero(&id);
        assert!(r.is_err());
    }

    #[test]
    fn test_collect_zero_refs() {
        let table = BlobRefcount::new();
        let id1 = make_blob_id(10);
        let id2 = make_blob_id(11);
        table.register(id1, 512, epoch(1)).unwrap();
        table.register(id2, 1024, epoch(1)).unwrap();
        table.dec(&id1, epoch(5)).unwrap(); // id1 -> ref=0
        let zeros = table.collect_zero_refs();
        assert_eq!(zeros.len(), 1);
        assert_eq!(zeros[0].0, id1);
    }

    #[test]
    fn test_stats() {
        let table = BlobRefcount::new();
        let id = make_blob_id(20);
        table.register(id, 512, epoch(1)).unwrap();
        table.inc(&id).unwrap();
        table.dec(&id, epoch(10)).unwrap();
        let s = table.stats();
        assert_eq!(s.blobs_registered, 1);
        assert_eq!(s.inc_total, 1);
        assert_eq!(s.dec_total, 1);
    }
}
