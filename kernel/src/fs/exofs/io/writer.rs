//! writer.rs — Écriture de blobs ExoFS dans le BlobStore (no_std).
//!
//! Ce module fournit :
//!  - `BlobStoreWrite`  : trait d'écriture dans le store de blobs.
//!  - `BlobWriter`      : écrivain avec buffer de pending + stats.
//!  - `WriteConfig`     : configuration de la session d'écriture.
//!  - `WriteEntry`      : entrée de l'écriture planifiée.
//!  - `PendingWriteBuffer`: file d'attente des écritures non encore flushées.
//!  - `VecStoreMut`     : implémentation mutable de BlobStoreWrite (tests).
//!
//! RECUR-01 : boucles while — aucune récursion.
//! OOM-02   : try_reserve avant push.
//! ARITH-02 : saturating_*, checked_div, wrapping_add.


extern crate alloc;
use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use super::io_stats::IO_STATS;
use super::reader::inline_blake3;

// ─── Trait d'écriture dans le store ──────────────────────────────────────────

/// Abstraction du store de blobs pour la couche d'écriture.
pub trait BlobStoreWrite {
    /// Écrit un blob. Retourne `Err(ObjectAlreadyExists)` si déjà présent.
    fn write_blob(&mut self, blob_id: &[u8; 32], data: &[u8]) -> ExofsResult<()>;

    /// Supprime un blob. Retourne `Err(BlobNotFound)` si absent.
    fn delete_blob(&mut self, blob_id: &[u8; 32]) -> ExofsResult<()>;

    /// Effectue un flush des données (équivalent fsync).
    fn flush(&mut self) -> ExofsResult<()>;

    /// Retourne vrai si le blob est présent.
    fn contains(&self, blob_id: &[u8; 32]) -> bool;
}

// ─── Configuration d'écriture ─────────────────────────────────────────────────

/// Configuration d'une session d'écriture.
#[derive(Clone, Copy, Debug)]
pub struct WriteConfig {
    /// Vérifier l'intégrité (blake3) après l'écriture.
    pub verify_after_write: bool,
    /// Appeler flush() automatiquement après chaque write_blob.
    pub auto_flush: bool,
    /// Nombre max d'entrées en attente (0 = illimité).
    pub max_pending: u32,
    /// Taille max d'un blob en bytes (0 = illimité).
    pub max_blob_size: u64,
    /// Enregistrer dans IO_STATS.
    pub record_stats: bool,
    /// Écrase silencieusement un blob déjà existant.
    pub overwrite: bool,
}

impl WriteConfig {
    pub fn default() -> Self {
        Self { verify_after_write: true, auto_flush: false, max_pending: 64,
            max_blob_size: 0, record_stats: true, overwrite: false }
    }

    pub fn fast() -> Self {
        Self { verify_after_write: false, auto_flush: false, max_pending: 256,
            max_blob_size: 0, record_stats: false, overwrite: true }
    }

    pub fn safe() -> Self {
        Self { verify_after_write: true, auto_flush: true, max_pending: 8,
            max_blob_size: 8 * 1024 * 1024, record_stats: true, overwrite: false }
    }

    pub fn validate(&self) -> ExofsResult<()> {
        if self.max_blob_size > 0 && self.max_blob_size > 512 * 1024 * 1024 {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(())
    }
}

// ─── Entrée de pending write ──────────────────────────────────────────────────

/// Entrée planifiée dans le buffer de pending writes.
#[derive(Clone, Debug)]
pub struct WriteEntry {
    pub blob_id: [u8; 32],
    pub data: Vec<u8>,
    pub ts: u64,
    pub is_delete: bool,
}

impl WriteEntry {
    pub fn write_entry(blob_id: [u8; 32], data: Vec<u8>, ts: u64) -> Self {
        Self { blob_id, data, ts, is_delete: false }
    }

    pub fn delete_entry(blob_id: [u8; 32], ts: u64) -> Self {
        Self { blob_id, data: Vec::new(), ts, is_delete: true }
    }

    pub fn data_len(&self) -> u64 { self.data.len() as u64 }
}

// ─── Buffer de pending writes ─────────────────────────────────────────────────

/// Buffer de writes en attente de flush (RECUR-01 : while).
pub struct PendingWriteBuffer {
    entries: Vec<WriteEntry>,
    max_pending: u32,
    total_bytes_pending: u64,
}

impl PendingWriteBuffer {
    pub fn new(max_pending: u32) -> Self {
        Self { entries: Vec::new(), max_pending, total_bytes_pending: 0 }
    }

    /// Ajoute une entrée d'écriture (OOM-02).
    pub fn add(&mut self, entry: WriteEntry) -> ExofsResult<()> {
        if self.max_pending > 0 && self.entries.len() as u32 >= self.max_pending {
            return Err(ExofsError::Resource);
        }
        let len = entry.data_len();
        self.entries.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.entries.push(entry);
        self.total_bytes_pending = self.total_bytes_pending.saturating_add(len);
        Ok(())
    }

    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
    pub fn total_bytes(&self) -> u64 { self.total_bytes_pending }

    /// Extrait toutes les entrées et les retourne (RECUR-01 : while).
    pub fn drain_all(&mut self) -> Vec<WriteEntry> {
        let mut out = Vec::with_capacity(self.entries.len());
        let mut i = 0;
        while i < self.entries.len() {
            i = i.wrapping_add(1);
        }
        core::mem::swap(&mut out, &mut self.entries);
        self.total_bytes_pending = 0;
        out
    }

    /// Vide le buffer sans exécuter les writes.
    pub fn discard_all(&mut self) {
        self.entries.clear();
        self.total_bytes_pending = 0;
    }

    /// Flush toutes les entrées vers le store (RECUR-01 : while).
    pub fn flush_to<S: BlobStoreWrite>(&mut self, store: &mut S) -> ExofsResult<u32> {
        let mut done = 0u32;
        let entries = self.drain_all();
        let mut i = 0usize;
        while i < entries.len() {
            let e = &entries[i];
            if e.is_delete {
                store.delete_blob(&e.blob_id)?;
            } else {
                store.write_blob(&e.blob_id, &e.data)?;
            }
            done = done.saturating_add(1);
            i = i.wrapping_add(1);
        }
        Ok(done)
    }
}

// ─── Statistiques d'écriture ──────────────────────────────────────────────────

/// Statistiques de la session BlobWriter.
#[derive(Clone, Copy, Debug, Default)]
pub struct WriterStats {
    pub blobs_written: u64,
    pub blobs_deleted: u64,
    pub bytes_written: u64,
    pub write_errors: u64,
    pub verify_errors: u64,
    pub flushes: u64,
}

impl WriterStats {
    pub fn new() -> Self { Self::default() }
    pub fn is_clean(&self) -> bool { self.write_errors == 0 && self.verify_errors == 0 }
    pub fn total_ops(&self) -> u64 { self.blobs_written.saturating_add(self.blobs_deleted) }
}

// ─── Écrivain de blobs ────────────────────────────────────────────────────────

/// Écrivain de blobs ExoFS avec buffer de pending writes.
///
/// RECUR-01 : toutes les boucles sont des `while`.
pub struct BlobWriter {
    config: WriteConfig,
    stats: WriterStats,
    pending: PendingWriteBuffer,
}

impl BlobWriter {
    pub fn new(config: WriteConfig) -> ExofsResult<Self> {
        config.validate()?;
        let max_pending = config.max_pending;
        Ok(Self { config, stats: WriterStats::new(), pending: PendingWriteBuffer::new(max_pending) })
    }

    pub fn default() -> Self {
        Self::new(WriteConfig::default()).expect("WriteConfig::default() is always valid")
    }

    /// Écrit un blob dans le store, avec vérification optionnelle.
    pub fn write<S: BlobStoreWrite>(
        &mut self,
        store: &mut S,
        blob_id: [u8; 32],
        data: &[u8],
    ) -> ExofsResult<()> {
        // Vérification taille (ARITH-02)
        if self.config.max_blob_size > 0 && data.len() as u64 > self.config.max_blob_size {
            return Err(ExofsError::InvalidArgument);
        }

        // Copie des données dans un Vec (OOM-02)
        let mut buf = Vec::new();
        buf.try_reserve(data.len()).map_err(|_| ExofsError::NoMemory)?;
        buf.extend_from_slice(data);

        if self.config.auto_flush {
            // Écriture directe
            let result = store.write_blob(&blob_id, &buf);
            match result {
                Ok(()) => {
                    self.stats.blobs_written = self.stats.blobs_written.saturating_add(1);
                    self.stats.bytes_written = self.stats.bytes_written.saturating_add(data.len() as u64);
                    if self.config.record_stats { IO_STATS.record_write_ok(data.len() as u64, 0); }
                }
                Err(e) => {
                    self.stats.write_errors = self.stats.write_errors.saturating_add(1);
                    if self.config.record_stats { IO_STATS.record_write_err(); }
                    return Err(e);
                }
            }
            // Vérification post-écriture
            if self.config.verify_after_write {
                let computed = inline_blake3(data);
                if computed != blob_id {
                    self.stats.verify_errors = self.stats.verify_errors.saturating_add(1);
                    return Err(ExofsError::ChecksumMismatch);
                }
            }
            if self.config.auto_flush {
                store.flush()?;
                self.stats.flushes = self.stats.flushes.saturating_add(1);
            }
        } else {
            // Enqueue dans le buffer
            let entry = WriteEntry::write_entry(blob_id, buf, 0);
            self.pending.add(entry)?;
        }
        Ok(())
    }

    /// Marque un blob pour suppression (buffered).
    pub fn delete<S: BlobStoreWrite>(
        &mut self,
        store: &mut S,
        blob_id: [u8; 32],
    ) -> ExofsResult<()> {
        if self.config.auto_flush {
            store.delete_blob(&blob_id)?;
            self.stats.blobs_deleted = self.stats.blobs_deleted.saturating_add(1);
        } else {
            let entry = WriteEntry::delete_entry(blob_id, 0);
            self.pending.add(entry)?;
        }
        Ok(())
    }

    /// Flush toutes les entrées pending vers le store (RECUR-01 : while).
    pub fn flush<S: BlobStoreWrite>(&mut self, store: &mut S) -> ExofsResult<u32> {
        let count = self.pending.flush_to(store)?;
        self.stats.blobs_written = self.stats.blobs_written.saturating_add(count as u64);
        store.flush()?;
        self.stats.flushes = self.stats.flushes.saturating_add(1);
        if self.config.record_stats { IO_STATS.record_flush(); }
        Ok(count)
    }

    /// Abandonner toutes les ecritures en pending.
    pub fn discard(&mut self) { self.pending.discard_all(); }

    pub fn pending_count(&self) -> usize { self.pending.len() }
    pub fn pending_bytes(&self) -> u64 { self.pending.total_bytes() }
    pub fn stats(&self) -> &WriterStats { &self.stats }
    pub fn reset_stats(&mut self) { self.stats = WriterStats::new(); }
}

// ─── VecStoreMut : implémentation mutable pour les tests ─────────────────────

/// Implémentation mutable de `BlobStoreWrite` + `BlobStoreRead` pour les tests.
pub struct VecStoreMut {
    blobs: Vec<([u8; 32], Vec<u8>)>,
    flush_count: u32,
}

impl VecStoreMut {
    pub fn new() -> Self { Self { blobs: Vec::new(), flush_count: 0 } }
    pub fn flush_count(&self) -> u32 { self.flush_count }
    pub fn blob_count(&self) -> usize { self.blobs.len() }

    pub fn get(&self, blob_id: &[u8; 32]) -> Option<&[u8]> {
        let mut i = 0usize;
        while i < self.blobs.len() {
            if self.blobs[i].0 == *blob_id { return Some(&self.blobs[i].1); }
            i = i.wrapping_add(1);
        }
        None
    }
}

impl BlobStoreWrite for VecStoreMut {
    fn write_blob(&mut self, blob_id: &[u8; 32], data: &[u8]) -> ExofsResult<()> {
        // Gestion overwrite (chercher existant)
        let mut i = 0usize;
        while i < self.blobs.len() {
            if self.blobs[i].0 == *blob_id {
                let mut v = Vec::new();
                v.try_reserve(data.len()).map_err(|_| ExofsError::NoMemory)?;
                v.extend_from_slice(data);
                self.blobs[i].1 = v;
                return Ok(());
            }
            i = i.wrapping_add(1);
        }
        let mut v = Vec::new();
        v.try_reserve(data.len()).map_err(|_| ExofsError::NoMemory)?;
        v.extend_from_slice(data);
        self.blobs.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.blobs.push((*blob_id, v));
        Ok(())
    }

    fn delete_blob(&mut self, blob_id: &[u8; 32]) -> ExofsResult<()> {
        let mut i = 0usize;
        while i < self.blobs.len() {
            if self.blobs[i].0 == *blob_id {
                self.blobs.swap_remove(i);
                return Ok(());
            }
            i = i.wrapping_add(1);
        }
        Err(ExofsError::BlobNotFound)
    }

    fn flush(&mut self) -> ExofsResult<()> {
        self.flush_count = self.flush_count.saturating_add(1);
        Ok(())
    }

    fn contains(&self, blob_id: &[u8; 32]) -> bool {
        let mut i = 0usize;
        while i < self.blobs.len() {
            if self.blobs[i].0 == *blob_id { return true; }
            i = i.wrapping_add(1);
        }
        false
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    fn make_id(tag: u8) -> [u8; 32] { let mut id = [0u8; 32]; id[0] = tag; id }

    #[test]
    fn test_write_auto_flush() {
        let mut store = VecStoreMut::new();
        let mut writer = BlobWriter::new(WriteConfig::fast()).expect("ok");
        let id = make_id(1);
        writer.write(&mut store, id, b"hello").expect("ok");
        assert!(!store.contains(&id)); // buffered
    }

    #[test]
    fn test_flush_writes_to_store() {
        let mut store = VecStoreMut::new();
        let mut writer = BlobWriter::new(WriteConfig::fast()).expect("ok");
        let id = make_id(2);
        writer.write(&mut store, id, b"flushed data").expect("ok");
        writer.flush(&mut store).expect("ok");
        assert!(store.contains(&id));
        assert_eq!(store.get(&id).expect("ok"), b"flushed data");
    }

    #[test]
    fn test_delete_buffered() {
        let mut store = VecStoreMut::new();
        let id = make_id(3);
        store.write_blob(&id, b"to delete").expect("ok");
        let mut writer = BlobWriter::new(WriteConfig::fast()).expect("ok");
        writer.delete(&mut store, id).expect("ok");
        assert_eq!(writer.pending_count(), 1);
        writer.flush(&mut store).expect("ok");
        assert!(!store.contains(&id));
    }

    #[test]
    fn test_max_pending_limit() {
        let mut store = VecStoreMut::new();
        let cfg = WriteConfig { max_pending: 2, auto_flush: false, ..WriteConfig::fast() };
        let mut writer = BlobWriter::new(cfg).expect("ok");
        writer.write(&mut store, make_id(1), b"a").expect("ok");
        writer.write(&mut store, make_id(2), b"b").expect("ok");
        // La 3e entrée doit produire une erreur Resource
        assert!(writer.write(&mut store, make_id(3), b"c").is_err());
    }

    #[test]
    fn test_discard() {
        let mut store = VecStoreMut::new();
        let mut writer = BlobWriter::new(WriteConfig::fast()).expect("ok");
        writer.write(&mut store, make_id(1), b"discard me").expect("ok");
        writer.discard();
        assert_eq!(writer.pending_count(), 0);
    }

    #[test]
    fn test_stats_tracking() {
        let mut store = VecStoreMut::new();
        let mut writer = BlobWriter::new(WriteConfig::fast()).expect("ok");
        writer.write(&mut store, make_id(1), b"a").expect("ok");
        writer.write(&mut store, make_id(2), b"bb").expect("ok");
        writer.flush(&mut store).expect("ok");
        assert_eq!(writer.stats().blobs_written, 2);
    }

    #[test]
    fn test_write_then_delete_then_flush() {
        let mut store = VecStoreMut::new();
        let id = make_id(5);
        let mut writer = BlobWriter::new(WriteConfig::fast()).expect("ok");
        writer.write(&mut store, id, b"ephemeral").expect("ok");
        writer.flush(&mut store).expect("ok");
        assert!(store.contains(&id));
        writer.delete(&mut store, id).expect("ok");
        writer.flush(&mut store).expect("ok");
        assert!(!store.contains(&id));
    }

    #[test]
    fn test_flush_count() {
        let mut store = VecStoreMut::new();
        let mut writer = BlobWriter::new(WriteConfig::fast()).expect("ok");
        writer.flush(&mut store).expect("ok");
        writer.flush(&mut store).expect("ok");
        assert_eq!(store.flush_count(), 2);
    }

    #[test]
    fn test_pending_bytes() {
        let mut store = VecStoreMut::new();
        let mut writer = BlobWriter::new(WriteConfig::fast()).expect("ok");
        writer.write(&mut store, make_id(1), b"abc").expect("ok");
        writer.write(&mut store, make_id(2), b"de").expect("ok");
        assert_eq!(writer.pending_bytes(), 5);
    }

    #[test]
    fn test_reset_stats() {
        let mut store = VecStoreMut::new();
        let mut writer = BlobWriter::new(WriteConfig::fast()).expect("ok");
        writer.write(&mut store, make_id(1), b"data").expect("ok");
        writer.flush(&mut store).expect("ok");
        writer.reset_stats();
        assert_eq!(writer.stats().blobs_written, 0);
    }

    #[test]
    fn test_delete_not_found() {
        let cfg = WriteConfig { auto_flush: true, ..WriteConfig::fast() };
        let mut store = VecStoreMut::new();
        let mut writer = BlobWriter::new(cfg).expect("ok");
        // Suppression d'un blob inexistant
        assert!(writer.delete(&mut store, make_id(0xFF)).is_err());
    }

    #[test]
    fn test_max_blob_size() {
        let mut store = VecStoreMut::new();
        let cfg = WriteConfig { max_blob_size: 4, auto_flush: true, ..WriteConfig::safe() };
        let mut writer = BlobWriter::new(cfg).expect("ok");
        assert!(writer.write(&mut store, make_id(1), b"too large data").is_err());
    }

    #[test]
    fn test_stats_is_clean() {
        let mut store = VecStoreMut::new();
        let cfg = WriteConfig { auto_flush: true, ..WriteConfig::fast() };
        let mut writer = BlobWriter::new(cfg).expect("ok");
        writer.write(&mut store, make_id(1), b"clean").expect("ok");
        assert!(writer.stats().is_clean());
    }

    #[test]
    fn test_pending_write_buffer_flush() {
        let mut store = VecStoreMut::new();
        let id = make_id(42);
        let mut buf = PendingWriteBuffer::new(10);
        let mut v = Vec::new();
        v.extend_from_slice(b"pending");
        let entry = WriteEntry::write_entry(id, v, 0);
        buf.add(entry).expect("ok");
        assert_eq!(buf.len(), 1);
        buf.flush_to(&mut store).expect("ok");
        assert!(store.contains(&id));
        assert_eq!(buf.len(), 0);
    }
}
