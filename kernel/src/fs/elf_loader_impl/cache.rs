//! cache.rs — Dedicated demand-paging ELF blob cache to avoid SLUB pressure

use alloc::sync::Arc;
use crate::fs::exofs::core::types::BlobId;
use crate::fs::exofs::syscall::object_store;
use crate::fs::exofs::syscall::path_resolve::resolve_path_to_blob;
use crate::memory::core::AllocError;
use crate::process::lifecycle::exec::ElfLoadError;
use spin::Mutex;

use super::trace::{trace_blob, trace_path};

pub const ELF_BLOB_CACHE_SLOTS: usize = 32;

pub struct ElfBlobCache {
    pub slots: [Option<(BlobId, Arc<[u8]>)>; ELF_BLOB_CACHE_SLOTS],
    pub next: usize,
}

static ELF_BLOB_CACHE: Mutex<ElfBlobCache> = Mutex::new(ElfBlobCache {
    slots: [const { None }; ELF_BLOB_CACHE_SLOTS],
    next: 0,
});

/// Bytes of an ELF blob: dedicated ELF cache -> disk (read + hashed ONLY once,
/// then reused for all subsequent page faults).
///
/// FIX #25 (Audit 3.4) — We NO LONGER query `BLOB_CACHE.get()` at all.
/// Reason: each `BLOB_CACHE.get()` writes `last_accessed`/`access_count` onto
/// a SLUB BTreeMap node and can allocate an `Arc<[u8]>` via
/// `materialize_snapshot`. This SLUB pressure during child ELF demand-paging
/// can force the buddy allocator to yield an ambiguous page. By reading
/// directly from `object_store::load_blob_data_if_available`, we eliminate
/// the "persisting class" identified in the audit.
pub fn load_blob_cached(blob_id: &BlobId) -> Result<Arc<[u8]>, AllocError> {
    // 1. Hit in dedicated ELF cache (Arc<[u8]> — no SLUB BTreeMap).
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
    // 2. Miss: direct read from object_store (NO BLOB_CACHE.get()).
    let data = object_store::load_blob_data_if_available(blob_id)
        .map_err(|_| AllocError::InvalidParams)?
        .ok_or(AllocError::InvalidParams)?;
    let arc: Arc<[u8]> = Arc::from(data.into_boxed_slice());
    // 3. Round-robin insertion (re-checks absence to avoid duplicates).
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

/// Resolves an ExoFS path to BlobId.
/// Delegates to resolve_path_to_blob() (pub(crate)) which computes Blake3 of canonical path.
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

/// Reads the full content of a blob from ExoFS disk.
///
/// FIX #25 (Audit 3.4) — We NO LONGER query `BLOB_CACHE.get()` at all.
pub fn read_blob_from_cache(blob_id: &BlobId) -> Result<Arc<[u8]>, ElfLoadError> {
    trace_blob(b"elf: cache read ", blob_id);
    load_blob_cached(blob_id).map_err(|_| ElfLoadError::NotFound)
}
