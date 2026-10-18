//! cache.rs — Cache de blobs ELF dédié au demand-paging pour éviter la pression SLUB

use alloc::sync::Arc;
use crate::fs::exofs::core::types::BlobId;
use crate::fs::exofs::syscall::object_store;
use crate::fs::exofs::syscall::path_resolve::resolve_path_to_blob;
use crate::memory::core::AllocError;
use crate::process::lifecycle::exec::ElfLoadError;
use spin::Mutex;

use super::trace::{trace_blob, trace_path};

pub const ELF_BLOB_CACHE_SLOTS: usize = 4;

pub struct ElfBlobCache {
    pub slots: [Option<(BlobId, Arc<[u8]>)>; ELF_BLOB_CACHE_SLOTS],
    pub next: usize,
}

static ELF_BLOB_CACHE: Mutex<ElfBlobCache> = Mutex::new(ElfBlobCache {
    slots: [None, None, None, None],
    next: 0,
});

/// Octets d'un blob ELF : cache ELF dédié → disque (lu + hashé UNE seule fois,
/// puis réutilisé pour toutes les fautes de page suivantes).
///
/// FIX #25 (Audit 3.4) — On ne consulte PLUS `BLOB_CACHE.get()` du tout.
/// Raison : chaque `BLOB_CACHE.get()` écrit `last_accessed`/`access_count` sur
/// un nœud BTreeMap SLUB et peut allouer un `Arc<[u8]>` via
/// `materialize_snapshot`. Cette pression SLUB pendant le demand-paging ELF
/// de l'enfant peut forcer le buddy à fournir une page ambiguë. En lisant
/// directement depuis `object_store::load_blob_data_if_available`, on élimine
/// la « classe qui persiste » identifiée dans l'audit.
pub fn load_blob_cached(blob_id: &BlobId) -> Result<Arc<[u8]>, AllocError> {
    // 1. Hit dans le cache ELF dédié (Arc<[u8]> — pas de BTreeMap SLUB).
    {
        let cache = ELF_BLOB_CACHE.lock();
        for slot in cache.slots.iter() {
            if let Some((id, arc)) = slot.as_ref() {
                if id == blob_id {
                    return Ok(Arc::clone(arc));
                }
            }
        }
    }
    // 2. Miss : lecture directe depuis object_store (PAS de BLOB_CACHE.get()).
    let data = object_store::load_blob_data_if_available(blob_id)
        .map_err(|_| AllocError::InvalidParams)?
        .ok_or(AllocError::InvalidParams)?;
    let arc: Arc<[u8]> = Arc::from(data.into_boxed_slice());
    // 3. Insertion round-robin (re-vérifie l'absence pour éviter un doublon).
    {
        let mut cache = ELF_BLOB_CACHE.lock();
        let already = cache
            .slots
            .iter()
            .any(|s| s.as_ref().map_or(false, |(id, _)| id == blob_id));
        if !already {
            let idx = cache.next % ELF_BLOB_CACHE_SLOTS;
            cache.next = cache.next.wrapping_add(1);
            cache.slots[idx] = Some((blob_id.clone(), Arc::clone(&arc)));
        }
    }
    Ok(arc)
}

/// Résout un chemin ExoFS en BlobId.
/// Délègue à resolve_path_to_blob() (pub(crate)) qui fait Blake3 du chemin canonique.
pub fn resolve_blob_id(path_bytes: &[u8]) -> Result<BlobId, ElfLoadError> {
    resolve_path_to_blob(path_bytes, 0)
        .map(|r| BlobId(r.blob_id))
        .map_err(|_| {
            trace_path(
                b"elf: resolve failed ",
                core::str::from_utf8(path_bytes).unwrap_or("<non-utf8>"),
            );
            ElfLoadError::NotFound
        })
}

/// Lit le contenu complet d'un blob depuis le disque ExoFS.
///
/// FIX #25 (Audit 3.4) — On ne consulte PLUS `BLOB_CACHE.get()` du tout.
pub fn read_blob_from_cache(blob_id: &BlobId) -> Result<Arc<[u8]>, ElfLoadError> {
    trace_blob(b"elf: disk direct ", blob_id);
    let Some(data) =
        object_store::load_blob_data_if_available(blob_id).map_err(|_| ElfLoadError::NotFound)?
    else {
        return Err(ElfLoadError::NotFound);
    };
    Ok(Arc::from(data.into_boxed_slice()))
}
