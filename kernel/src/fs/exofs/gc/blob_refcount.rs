//! Comptage de références pour les P-Blobs (Physical Blobs).
//!
//! RÈGLE 12 : ref_count P-Blob : panic si underflow (jamais fetch_sub aveugle).
//! RÈGLE 14 : checked_add() pour TOUS calculs d'offsets/index.

use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::fs::exofs::core::{BlobId, FsError};
use crate::scheduler::sync::spinlock::SpinLock;

/// Entrée de comptage pour un P-Blob.
#[derive(Debug)]
struct RefEntry {
    /// Compteur de références live.
    count: AtomicU32,
    /// Taille physique du blob en bytes (pour les métriques de freed_bytes).
    phys_size: u64,
}

impl RefEntry {
    fn new(initial: u32, phys_size: u64) -> Self {
        Self {
            count: AtomicU32::new(initial),
            phys_size,
        }
    }
}

/// Table globale des compteurs de références P-Blob.
///
/// Protégée par SpinLock pour les mutations structurelles (insert/remove).
/// Les incréments/décréments atomiques ne nécessitent pas le lock.
pub struct BlobRefcount {
    inner: SpinLock<BTreeMap<BlobId, RefEntry>>,
}

impl BlobRefcount {
    pub const fn new() -> Self {
        Self {
            inner: SpinLock::new(BTreeMap::new()),
        }
    }

    /// Enregistre un nouveau blob avec un compteur initial (typiquement 1).
    pub fn register(
        &self,
        id: BlobId,
        initial: u32,
        phys_size: u64,
    ) -> Result<(), FsError> {
        let mut map = self.inner.lock();
        if map.contains_key(&id) {
            return Err(FsError::AlreadyExists);
        }
        map.try_reserve(1)
            .map_err(|_| FsError::OutOfMemory)?;
        map.insert(id, RefEntry::new(initial, phys_size));
        Ok(())
    }

    /// Incrémente le compteur. Retourne le nouveau compteur.
    /// Retourne `FsError::NotFound` si le blob n'est pas enregistré.
    pub fn inc(&self, id: &BlobId) -> Result<u32, FsError> {
        let map = self.inner.lock();
        match map.get(id) {
            Some(e) => {
                // Overflow théoriquement impossible (limites metadata), mais check quand même.
                let old = e.count.fetch_add(1, Ordering::AcqRel);
                old.checked_add(1)
                    .ok_or(FsError::Overflow)?;
                Ok(old + 1)
            }
            None => Err(FsError::NotFound),
        }
    }

    /// Décrémente le compteur.
    ///
    /// RÈGLE 12 : PANIC si underflow.
    /// Retourne `(nouveau_count, phys_size)`.
    /// Si `nouveau_count == 0`, le blob est candidat à la suppression.
    pub fn dec(&self, id: &BlobId) -> Result<(u32, u64), FsError> {
        let map = self.inner.lock();
        match map.get(id) {
            Some(e) => {
                let old = e.count.load(Ordering::Acquire);
                if old == 0 {
                    // RÈGLE 12 : jamais fetch_sub aveugle → panic.
                    panic!(
                        "[ExoFS BlobRefcount] underflow sur BlobId {:?} — invariant GC violé",
                        id
                    );
                }
                let new_count = e.count.fetch_sub(1, Ordering::AcqRel) - 1;
                Ok((new_count, e.phys_size))
            }
            None => Err(FsError::NotFound),
        }
    }

    /// Retourne le compteur courant sans le modifier.
    pub fn get(&self, id: &BlobId) -> Option<u32> {
        let map = self.inner.lock();
        map.get(id).map(|e| e.count.load(Ordering::Acquire))
    }

    /// Supprime l'entrée d'un blob dont le compteur est 0.
    /// Retourne `FsError::BusBusy` si le compteur n'est pas 0.
    pub fn remove_zero(&self, id: &BlobId) -> Result<u64, FsError> {
        let mut map = self.inner.lock();
        match map.get(id) {
            Some(e) if e.count.load(Ordering::Acquire) == 0 => {
                let phys_size = e.phys_size;
                map.remove(id);
                Ok(phys_size)
            }
            Some(_) => Err(FsError::RefCountNonZero),
            None => Err(FsError::NotFound),
        }
    }

    /// Retourne le nombre total de P-Blobs suivis.
    pub fn blob_count(&self) -> usize {
        self.inner.lock().len()
    }

    /// Itère sur tous les blobs dont le compteur == 0 → candidats GC.
    pub fn collect_zero_refs<F>(&self, mut f: F)
    where
        F: FnMut(BlobId, u64),
    {
        let map = self.inner.lock();
        for (id, entry) in map.iter() {
            if entry.count.load(Ordering::Acquire) == 0 {
                f(*id, entry.phys_size);
            }
        }
    }
}

/// Instance globale de la table de référence.
pub static BLOB_REFCOUNT: BlobRefcount = BlobRefcount::new();
