//! Zero-copy IO ExoFS — transfert de données sans copie intermédiaire.
//!
//! Expose une vue read-only sur les données d'un blob directement dans le
//! cache blob (page frame mapping).
//!
//! RÈGLE 3  : tout unsafe → // SAFETY: <raison>
//! RÈGLE 9  : copy_from_user() obligatoire pour pointeurs userspace.

use core::ops::Deref;

use crate::fs::exofs::core::{BlobId, FsError};
use crate::fs::exofs::storage::BlobStore;

/// Slice zero-copy sur les données d'un blob.
///
/// Maintient une référence de page-frame bloquant son éviction du cache.
pub struct ZeroCopySlice<'store> {
    _store: &'store BlobStore,
    /// Pointeur vers les données mappées en kernel space.
    ptr: *const u8,
    /// Longueur du slice.
    len: usize,
    /// Offset dans le blob.
    pub offset: u64,
    /// BlobId source.
    pub blob_id: BlobId,
}

// SAFETY: Le pointeur `ptr` pointe vers une page frame kernel épinglée
// pour la durée de vie `'store`. Aucun thread userspace ne peut y accéder.
unsafe impl<'store> Send for ZeroCopySlice<'store> {}
unsafe impl<'store> Sync for ZeroCopySlice<'store> {}

impl<'store> ZeroCopySlice<'store> {
    /// Mappe une plage du blob en lecture zero-copy.
    ///
    /// Retourne une erreur si la plage n'est pas dans le cache ou si le blob
    /// n'est pas éligible au zero-copy (compressé, chiffré, etc.).
    pub fn map(
        store: &'store BlobStore,
        blob_id: BlobId,
        offset: u64,
        len: usize,
    ) -> Result<Self, FsError> {
        // Obtient un pointeur direct vers la page frame dans le blob cache.
        let ptr = store.map_blob_range_readonly(&blob_id, offset, len)?;
        if ptr.is_null() {
            return Err(FsError::NullPointer);
        }
        Ok(Self { _store: store, ptr, len, offset, blob_id })
    }

    /// Retourne la longueur du slice.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl<'store> Deref for ZeroCopySlice<'store> {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        // SAFETY: `ptr` est une page frame kernel épinglée, valide pour 'store,
        // non muable depuis userspace, et la longueur est vérifiée à la création.
        unsafe { core::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl<'store> Drop for ZeroCopySlice<'store> {
    fn drop(&mut self) {
        // Dépin de la page frame dans le cache.
        // SAFETY: blob_id et offset sont ceux fournis à la création.
        unsafe {
            self._store.unmap_blob_range(self.blob_id, self.offset, self.len);
        }
    }
}

/// Copie zero-copy kernel→kernel entre deux plages de blobs.
///
/// Utilise le mapping direct sans buffer intermédiaire.
pub fn copy_blob_range<'s>(
    store: &'s BlobStore,
    src_id: BlobId,
    src_offset: u64,
    dst_id: BlobId,
    dst_offset: u64,
    len: usize,
) -> Result<(), FsError> {
    let src = ZeroCopySlice::map(store, src_id, src_offset, len)?;
    // SAFETY: src et dst blobs sont distincts (src_id != dst_id dans le caller),
    // les plages ne se chevauchent pas, et le store garantit l'isolation.
    store.write_blob_at(&dst_id, dst_offset, &src)?;
    Ok(())
}
