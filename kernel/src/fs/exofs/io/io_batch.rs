//! io_batch.rs — Traitement par lots d'opérations IO (no_std).
//!
//! Ce module fournit :
//!  - `IoBatchEntry`   : entrée d'un lot (blob_id + opération + range).
//!  - `IoBatch`        : lot d'opérations avec add_read/add_write/execute.
//!  - `BatchResult`    : résultat de l'exécution d'un lot.
//!  - `BatchStats`     : statistiques cumulées de plusieurs lots.
//!
//! RECUR-01 : boucles while — aucune récursion.
//! OOM-02   : try_reserve avant push.
//! ARITH-02 : saturating_*, checked_div, wrapping_add/mul.


extern crate alloc;
use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use super::io_stats::IoOpKind;

// ─── IoBatchEntry ─────────────────────────────────────────────────────────────

/// Une entrée dans un lot d'opérations IO.
#[derive(Clone, Debug)]
pub struct IoBatchEntry {
    pub blob_id:    [u8; 32],
    pub op:         IoOpKind,
    pub buf_offset: u32,       // offset dans l'entrée (lecture partielle)
    pub buf_len:    u32,       // nombre d'octets à traiter (0 = tout)
    pub priority:   u8,        // 0 = normal, 255 = urgent
}

impl IoBatchEntry {
    pub fn read(blob_id: [u8; 32]) -> Self {
        Self { blob_id, op: IoOpKind::Read, buf_offset: 0, buf_len: 0, priority: 0 }
    }

    pub fn write(blob_id: [u8; 32], len: u32) -> Self {
        Self { blob_id, op: IoOpKind::Write, buf_offset: 0, buf_len: len, priority: 0 }
    }

    pub fn read_partial(blob_id: [u8; 32], offset: u32, len: u32) -> Self {
        Self { blob_id, op: IoOpKind::Read, buf_offset: offset, buf_len: len, priority: 0 }
    }

    pub fn with_priority(mut self, p: u8) -> Self { self.priority = p; self }

    pub fn is_read(&self)  -> bool { self.op.is_read() }
    pub fn is_write(&self) -> bool { self.op.is_write() }
}

// ─── BatchResult ─────────────────────────────────────────────────────────────

/// Résultat de l'exécution d'un lot.
#[derive(Clone, Debug, Default)]
pub struct BatchResult {
    pub entries_ok:  u32,
    pub entries_err: u32,
    pub bytes_total: u64,
    pub errors:      Vec<(usize, ExofsError)>,  // (index, err)
}

impl BatchResult {
    pub fn new() -> Self { Self::default() }

    pub fn add_ok(&mut self, bytes: u64) {
        self.entries_ok = self.entries_ok.saturating_add(1);
        self.bytes_total = self.bytes_total.saturating_add(bytes);
    }

    pub fn add_err(&mut self, idx: usize, e: ExofsError) -> ExofsResult<()> {
        self.entries_err = self.entries_err.saturating_add(1);
        self.errors.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.errors.push((idx, e));
        Ok(())
    }

    pub fn is_all_ok(&self) -> bool { self.entries_err == 0 }
    pub fn total_entries(&self) -> u32 { self.entries_ok.saturating_add(self.entries_err) }

    pub fn success_rate_pct10(&self) -> u32 {
        let total = self.total_entries();
        if total == 0 { return 1000; }
        (self.entries_ok as u32).saturating_mul(1000)
            .checked_div(total)
            .unwrap_or(0)
    }
}

// ─── BatchStats ──────────────────────────────────────────────────────────────

/// Statistiques cumulées de plusieurs lots.
#[derive(Clone, Copy, Debug, Default)]
pub struct BatchStats {
    pub batches_executed: u64,
    pub entries_total:    u64,
    pub entries_ok:       u64,
    pub entries_err:      u64,
    pub bytes_total:      u64,
}

impl BatchStats {
    pub fn new() -> Self { Self::default() }

    pub fn accumulate(&mut self, result: &BatchResult) {
        self.batches_executed = self.batches_executed.saturating_add(1);
        self.entries_total    = self.entries_total.saturating_add(result.total_entries() as u64);
        self.entries_ok       = self.entries_ok.saturating_add(result.entries_ok as u64);
        self.entries_err      = self.entries_err.saturating_add(result.entries_err as u64);
        self.bytes_total      = self.bytes_total.saturating_add(result.bytes_total);
    }

    pub fn is_clean(&self) -> bool { self.entries_err == 0 }
    pub fn reset(&mut self) { *self = Self::new(); }
}

// ─── Store trait pour les tests de IoBatch ────────────────────────────────────

/// Trait d'accès minimal au store utilisé par IoBatch.
pub trait BatchStore {
    fn read(&self, blob_id: &[u8; 32], offset: u32, len: u32) -> ExofsResult<(Vec<u8>, u64)>;
    fn write(&mut self, blob_id: &[u8; 32], data: &[u8]) -> ExofsResult<u64>;
}

// ─── VecBatchStore (tests) ────────────────────────────────────────────────────

/// Implémentation BatchStore sur Vec pour les tests.
pub struct VecBatchStore {
    blobs: Vec<([u8; 32], Vec<u8>)>,
}

impl VecBatchStore {
    pub fn new() -> Self { Self { blobs: Vec::new() } }

    pub fn insert(&mut self, id: [u8; 32], data: &[u8]) -> ExofsResult<()> {
        let mut v = Vec::new();
        v.try_reserve(data.len()).map_err(|_| ExofsError::NoMemory)?;
        v.extend_from_slice(data);
        self.blobs.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.blobs.push((id, v));
        Ok(())
    }
}

impl BatchStore for VecBatchStore {
    fn read(&self, blob_id: &[u8; 32], offset: u32, len: u32) -> ExofsResult<(Vec<u8>, u64)> {
        let mut i = 0usize;
        while i < self.blobs.len() {
            if self.blobs[i].0 == *blob_id {
                let data = &self.blobs[i].1;
                let start = (offset as usize).min(data.len());
                let end = if len == 0 { data.len() }
                    else { start.saturating_add(len as usize).min(data.len()) };
                let mut out = Vec::new();
                out.try_reserve(end - start).map_err(|_| ExofsError::NoMemory)?;
                out.extend_from_slice(&data[start..end]);
                let bytes = (end - start) as u64;
                return Ok((out, bytes));
            }
            i = i.wrapping_add(1);
        }
        Err(ExofsError::BlobNotFound)
    }

    fn write(&mut self, blob_id: &[u8; 32], data: &[u8]) -> ExofsResult<u64> {
        let mut i = 0usize;
        while i < self.blobs.len() {
            if self.blobs[i].0 == *blob_id {
                let mut v = Vec::new();
                v.try_reserve(data.len()).map_err(|_| ExofsError::NoMemory)?;
                v.extend_from_slice(data);
                self.blobs[i].1 = v;
                return Ok(data.len() as u64);
            }
            i = i.wrapping_add(1);
        }
        let mut v = Vec::new();
        v.try_reserve(data.len()).map_err(|_| ExofsError::NoMemory)?;
        v.extend_from_slice(data);
        self.blobs.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.blobs.push((*blob_id, v));
        Ok(data.len() as u64)
    }
}

// ─── IoBatch ─────────────────────────────────────────────────────────────────

/// Lot d'opérations IO à exécuter collectivement.
///
/// RECUR-01 : toutes les boucles while.
pub struct IoBatch {
    entries:   Vec<IoBatchEntry>,
    max_entries: u32,
    stats:     BatchStats,
    write_data: Vec<Vec<u8>>,  // données associées aux writes
}

impl IoBatch {
    pub fn new(max_entries: u32) -> Self {
        Self { entries: Vec::new(), max_entries, stats: BatchStats::new(), write_data: Vec::new() }
    }

    pub fn default() -> Self { Self::new(64) }

    pub fn entry_count(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
    pub fn stats(&self) -> &BatchStats { &self.stats }

    /// Ajoute une lecture (OOM-02).
    pub fn add_read(&mut self, blob_id: [u8; 32]) -> ExofsResult<()> {
        self.check_capacity()?;
        self.entries.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.write_data.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.entries.push(IoBatchEntry::read(blob_id));
        self.write_data.push(Vec::new());
        Ok(())
    }

    /// Ajoute une écriture avec données (OOM-02).
    pub fn add_write(&mut self, blob_id: [u8; 32], data: &[u8]) -> ExofsResult<()> {
        self.check_capacity()?;
        let mut v = Vec::new();
        v.try_reserve(data.len()).map_err(|_| ExofsError::NoMemory)?;
        v.extend_from_slice(data);
        self.entries.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.write_data.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.entries.push(IoBatchEntry::write(blob_id, data.len() as u32));
        self.write_data.push(v);
        Ok(())
    }

    fn check_capacity(&self) -> ExofsResult<()> {
        if self.max_entries > 0 && self.entries.len() as u32 >= self.max_entries {
            return Err(ExofsError::Resource);
        }
        Ok(())
    }

    /// Exécute toutes les entrées du lot (RECUR-01 : while).
    pub fn execute<S: BatchStore>(&mut self, store: &mut S) -> ExofsResult<BatchResult> {
        let mut result = BatchResult::new();
        let mut i = 0usize;
        while i < self.entries.len() {
            let entry = &self.entries[i];
            match entry.op.is_read() {
                true => {
                    match store.read(&entry.blob_id, entry.buf_offset, entry.buf_len) {
                        Ok((_, bytes)) => result.add_ok(bytes),
                        Err(e) => result.add_err(i, e)?,
                    }
                }
                false => {
                    let data = &self.write_data[i];
                    match store.write(&entry.blob_id, data) {
                        Ok(bytes) => result.add_ok(bytes),
                        Err(e) => result.add_err(i, e)?,
                    }
                }
            }
            i = i.wrapping_add(1);
        }
        self.stats.accumulate(&result);
        Ok(result)
    }

    /// Vide le lot (sans réinitialiser les stats).
    pub fn clear(&mut self) { self.entries.clear(); self.write_data.clear(); }
}

// ─── Tests ───────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    fn make_id(n: u8) -> [u8; 32] { let mut id = [0u8; 32]; id[0] = n; id }

    fn make_store() -> VecBatchStore {
        let mut store = VecBatchStore::new();
        store.insert(make_id(1), b"hello batch").expect("ok");
        store.insert(make_id(2), b"world batch").expect("ok");
        store
    }

    #[test]
    fn test_batch_read() {
        let mut store = make_store();
        let mut batch = IoBatch::default();
        batch.add_read(make_id(1)).expect("ok");
        batch.add_read(make_id(2)).expect("ok");
        let result = batch.execute(&mut store).expect("ok");
        assert_eq!(result.entries_ok, 2);
        assert!(result.is_all_ok());
    }

    #[test]
    fn test_batch_write() {
        let mut store = VecBatchStore::new();
        let mut batch = IoBatch::default();
        batch.add_write(make_id(3), b"new entry").expect("ok");
        let result = batch.execute(&mut store).expect("ok");
        assert_eq!(result.entries_ok, 1);
    }

    #[test]
    fn test_batch_mixed() {
        let mut store = make_store();
        let mut batch = IoBatch::default();
        batch.add_read(make_id(1)).expect("ok");
        batch.add_write(make_id(3), b"write data").expect("ok");
        let result = batch.execute(&mut store).expect("ok");
        assert_eq!(result.entries_ok, 2);
    }

    #[test]
    fn test_batch_not_found() {
        let mut store = VecBatchStore::new();
        let mut batch = IoBatch::default();
        batch.add_read(make_id(99)).expect("ok");
        let result = batch.execute(&mut store).expect("ok");
        assert_eq!(result.entries_err, 1);
        assert!(!result.is_all_ok());
    }

    #[test]
    fn test_batch_max_entries() {
        let mut batch = IoBatch::new(2);
        batch.add_read(make_id(1)).expect("ok");
        batch.add_read(make_id(2)).expect("ok");
        assert!(batch.add_read(make_id(3)).is_err());
    }

    #[test]
    fn test_batch_stats_accumulate() {
        let mut store = make_store();
        let mut batch = IoBatch::default();
        batch.add_read(make_id(1)).expect("ok");
        batch.execute(&mut store).expect("ok");
        assert_eq!(batch.stats().batches_executed, 1);
        assert_eq!(batch.stats().entries_ok, 1);
    }

    #[test]
    fn test_batch_clear() {
        let mut batch = IoBatch::default();
        batch.add_read(make_id(1)).expect("ok");
        batch.clear();
        assert!(batch.is_empty());
    }

    #[test]
    fn test_batch_result_success_rate() {
        let mut r = BatchResult::new();
        r.add_ok(100);
        r.add_ok(200);
        assert_eq!(r.success_rate_pct10(), 1000); // 100%
    }

    #[test]
    fn test_batch_result_partial_success_rate() {
        let mut r = BatchResult::new();
        r.add_ok(100);
        r.add_err(1, ExofsError::BlobNotFound).expect("ok");
        assert_eq!(r.success_rate_pct10(), 500); // 50%
    }

    #[test]
    fn test_batch_entry_priority() {
        let e = IoBatchEntry::read(make_id(1)).with_priority(255);
        assert_eq!(e.priority, 255);
    }

    #[test]
    fn test_io_batch_entry_is_read_write() {
        let r = IoBatchEntry::read(make_id(1));
        let w = IoBatchEntry::write(make_id(2), 100);
        assert!(r.is_read());
        assert!(!r.is_write());
        assert!(w.is_write());
        assert!(!w.is_read());
    }

    #[test]
    fn test_vec_batch_store_read_partial() {
        let mut store = VecBatchStore::new();
        store.insert(make_id(1), b"hello world").expect("ok");
        let (data, bytes) = store.read(&make_id(1), 6, 5).expect("ok");
        assert_eq!(data, b"world");
        assert_eq!(bytes, 5);
    }

    #[test]
    fn test_stats_reset() {
        let mut stats = BatchStats::new();
        let r = BatchResult { entries_ok: 3, entries_err: 1, bytes_total: 100, errors: Vec::new() };
        stats.accumulate(&r);
        stats.reset();
        assert_eq!(stats.batches_executed, 0);
    }
}

// ─── Utilitaires complémentaires ─────────────────────────────────────────────

/// Retourne le nombre d'octets estimé pour un lot de lectures (heuristique).
pub fn estimate_batch_bytes(entries: &[IoBatchEntry], avg_blob_size: u32) -> u64 {
    let mut total = 0u64;
    let mut i = 0usize;
    while i < entries.len() {
        if entries[i].is_read() {
            total = total.saturating_add(avg_blob_size as u64);
        }
        i = i.wrapping_add(1);
    }
    total
}
