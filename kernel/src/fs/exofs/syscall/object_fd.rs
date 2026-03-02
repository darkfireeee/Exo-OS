//! ObjectFd — table de descripteurs de fichiers ExoFS pour les syscalls (no_std).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::{BlobId, FsError};
use crate::fs::exofs::path::resolver::PathResolver;
use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;

pub static OBJECT_TABLE: ObjectFdTable = ObjectFdTable::new_const();

/// Descripteur d'objet ouvert.
#[derive(Clone, Debug)]
pub struct ObjectFd {
    pub fd:       u32,
    pub blob_id:  BlobId,
    pub flags:    u32,
    pub cursor:   u64,
}

pub struct ObjectFdTable {
    table:   SpinLock<BTreeMap<u32, ObjectFd>>,
    next_fd: AtomicU32,
}

impl ObjectFdTable {
    pub const fn new_const() -> Self {
        Self {
            table:   SpinLock::new(BTreeMap::new()),
            next_fd: AtomicU32::new(4), // 0-3 réservés stdin/out/err/exofs
        }
    }

    pub fn open(&self, path: &str, flags: u32) -> Result<u32, FsError> {
        let _object_id = PathResolver::resolve(path)?;
        let blob_id = BlobId::from_bytes_blake3(path.as_bytes());
        let fd = self.next_fd.fetch_add(1, Ordering::Relaxed);
        let mut table = self.table.lock();
        table.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        table.insert(fd, ObjectFd { fd, blob_id, flags, cursor: 0 });
        Ok(fd)
    }

    pub fn read(&self, fd: u32, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        let blob_id = {
            let table = self.table.lock();
            table.get(&fd).map(|f| f.blob_id).ok_or(FsError::NotFound)?
        };
        if let Some(data) = BLOB_CACHE.get(&blob_id) {
            let start = offset as usize;
            if start >= data.len() { return Ok(0); }
            let end = (start + buf.len()).min(data.len());
            let n = end - start;
            buf[..n].copy_from_slice(&data[start..end]);
            Ok(n)
        } else {
            Err(FsError::NotFound)
        }
    }

    pub fn write(&self, fd: u32, offset: u64, data: &[u8]) -> Result<usize, FsError> {
        let blob_id = {
            let table = self.table.lock();
            table.get(&fd).map(|f| f.blob_id).ok_or(FsError::NotFound)?
        };
        // Écriture via cache blob.
        let to_insert: alloc::boxed::Box<[u8]> = data.into();
        BLOB_CACHE.insert(blob_id, data)?;
        BLOB_CACHE.mark_dirty(&blob_id);
        Ok(data.len())
    }

    pub fn create(&self, path: &str, flags: u32) -> Result<u64, FsError> {
        // Le BlobId est dérivé du chemin (RÈGLE 11 : Blake3 des données brutes).
        // Ici le "contenu brut" pour un nouvel objet vide est le chemin canonique.
        let blob_id = BlobId::from_bytes_blake3(path.as_bytes());
        let fd = self.next_fd.fetch_add(1, Ordering::Relaxed);
        let mut table = self.table.lock();
        table.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        table.insert(fd, ObjectFd { fd, blob_id, flags, cursor: 0 });
        // Retourne l'id numérique (premiers 8 octets du BlobId).
        let bytes = blob_id.as_bytes();
        let id = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
        Ok(id)
    }

    pub fn delete(&self, path: &str) -> Result<(), FsError> {
        let blob_id = BlobId::from_bytes_blake3(path.as_bytes());
        BLOB_CACHE.invalidate(&blob_id);
        Ok(())
    }

    pub fn close(&self, fd: u32) -> bool {
        self.table.lock().remove(&fd).is_some()
    }

    pub fn get_blob_id(&self, fd: u32) -> Option<BlobId> {
        self.table.lock().get(&fd).map(|f| f.blob_id)
    }
}
