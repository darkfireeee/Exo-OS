// kernel/src/fs/exofs/storage/dedup_writer.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Déduplication à l'écriture — ExoFS
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// DedupWriter consulte un index de BlobId avant chaque écriture.
// Si le blob est déjà présent, l'écriture est supprimée et le BlobId existant
// est retourné (dedup hit). Sinon, le blob est écrit et indexé.
//
// Règles ExoFS :
// - HASH-02  : BlobId = Blake3 sur données BRUTES (avant compression).
// - OOM-02   : try_reserve avant toute insertion.
// - ARITH-02 : checked_add pour l'offset disque.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::{ExofsError, ExofsResult, BlobId, DiskOffset};
use crate::fs::exofs::core::blob_id::compute_blob_id;
use crate::fs::exofs::storage::storage_stats::STORAGE_STATS;

// ─────────────────────────────────────────────────────────────────────────────
// DedupEntry — entrée dans l'index de déduplication
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct DedupEntry {
    pub blob_id:   BlobId,
    pub offset:    DiskOffset,
    pub size:      u64,
    pub ref_count: u32,
}

impl DedupEntry {
    pub fn new(blob_id: BlobId, offset: DiskOffset, size: u64) -> Self {
        Self { blob_id, offset, size, ref_count: 1 }
    }

    pub fn inc_ref(&mut self) {
        self.ref_count = self.ref_count.saturating_add(1);
    }

    pub fn dec_ref(&mut self) -> u32 {
        self.ref_count = self.ref_count.saturating_sub(1);
        self.ref_count
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DedupDecision — résultat de la consultation de l'index
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum DedupDecision {
    /// Blob déjà présent — écriture inutile.
    Hit {
        blob_id: BlobId,
        offset:  DiskOffset,
        saved:   u64,
    },
    /// Blob nouveau — doit être écrit.
    Miss {
        blob_id: BlobId,
    },
}

impl DedupDecision {
    pub fn is_hit(&self)  -> bool { matches!(self, Self::Hit  { .. }) }
    pub fn is_miss(&self) -> bool { matches!(self, Self::Miss { .. }) }

    pub fn blob_id(&self) -> &BlobId {
        match self {
            Self::Hit  { blob_id, .. } => blob_id,
            Self::Miss { blob_id }     => blob_id,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DedupIndex — table de hachage légère indexée par BlobId
// ─────────────────────────────────────────────────────────────────────────────

const DEDUP_BUCKETS: usize = 256;

struct DedupBucket {
    entries: Vec<DedupEntry>,
}

impl DedupBucket {
    const fn new() -> Self { Self { entries: Vec::new() } }

    fn find(&self, id: &BlobId) -> Option<&DedupEntry> {
        self.entries.iter().find(|e| e.blob_id == *id)
    }

    fn find_mut(&mut self, id: &BlobId) -> Option<&mut DedupEntry> {
        self.entries.iter_mut().find(|e| e.blob_id == *id)
    }

    fn insert(&mut self, entry: DedupEntry) -> ExofsResult<()> {
        self.entries.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.entries.push(entry);
        Ok(())
    }

    fn remove(&mut self, id: &BlobId) -> Option<DedupEntry> {
        if let Some(pos) = self.entries.iter().position(|e| e.blob_id == *id) {
            Some(self.entries.swap_remove(pos))
        } else {
            None
        }
    }

    fn len(&self) -> usize { self.entries.len() }
}

struct DedupIndexInner {
    buckets:   [DedupBucket; DEDUP_BUCKETS],
    total:     usize,
}

impl DedupIndexInner {
    fn new() -> Self {
        // Initialiser les 256 buckets.
        const EMPTY: DedupBucket = DedupBucket { entries: Vec::new() };
        Self { buckets: [EMPTY; DEDUP_BUCKETS], total: 0 }
    }

    fn bucket_index(id: &BlobId) -> usize {
        id.0[0] as usize
    }

    fn lookup(&self, id: &BlobId) -> Option<&DedupEntry> {
        let idx = Self::bucket_index(id);
        self.buckets[idx].find(id)
    }

    fn insert(&mut self, entry: DedupEntry) -> ExofsResult<()> {
        let idx = Self::bucket_index(&entry.blob_id);
        self.buckets[idx].insert(entry)?;
        self.total = self.total.saturating_add(1);
        Ok(())
    }

    fn inc_ref(&mut self, id: &BlobId) -> bool {
        let idx = Self::bucket_index(id);
        if let Some(e) = self.buckets[idx].find_mut(id) {
            e.inc_ref();
            true
        } else {
            false
        }
    }

    fn dec_ref(&mut self, id: &BlobId) -> Option<u32> {
        let idx = Self::bucket_index(id);
        if let Some(e) = self.buckets[idx].find_mut(id) {
            let remaining = e.dec_ref();
            Some(remaining)
        } else {
            None
        }
    }

    fn remove(&mut self, id: &BlobId) -> Option<DedupEntry> {
        let idx = Self::bucket_index(id);
        let r   = self.buckets[idx].remove(id);
        if r.is_some() { self.total = self.total.saturating_sub(1); }
        r
    }

    fn total_entries(&self) -> usize { self.total }

    fn load_factor_pct(&self) -> u32 {
        if DEDUP_BUCKETS == 0 { return 0; }
        (self.total * 100 / DEDUP_BUCKETS) as u32
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DedupWriter
// ─────────────────────────────────────────────────────────────────────────────

pub struct DedupWriter {
    index:  SpinLock<DedupIndexInner>,
    hits:   AtomicU64,
    misses: AtomicU64,
    saved:  AtomicU64,
}

impl DedupWriter {
    pub fn new() -> Self {
        Self {
            index:  SpinLock::new(DedupIndexInner::new()),
            hits:   AtomicU64::new(0),
            misses: AtomicU64::new(0),
            saved:  AtomicU64::new(0),
        }
    }

    /// Consulte l'index pour des données brutes.
    ///
    /// # Règle HASH-02 : `raw_data` est le contenu AVANT compression.
    pub fn check(&self, raw_data: &[u8]) -> DedupDecision {
        let blob_id = compute_blob_id(raw_data);
        let guard   = self.index.lock();

        if let Some(entry) = guard.lookup(&blob_id) {
            let offset = entry.offset;
            let saved  = raw_data.len() as u64;
            drop(guard);
            self.hits.fetch_add(1, Ordering::Relaxed);
            self.saved.fetch_add(saved, Ordering::Relaxed);
            STORAGE_STATS.inc_dedup_hit(saved);
            DedupDecision::Hit { blob_id, offset, saved }
        } else {
            drop(guard);
            self.misses.fetch_add(1, Ordering::Relaxed);
            STORAGE_STATS.inc_dedup_miss();
            DedupDecision::Miss { blob_id }
        }
    }

    /// Enregistre un blob nouvellement écrit dans l'index.
    pub fn register(
        &self,
        blob_id: BlobId,
        offset:  DiskOffset,
        size:    u64,
    ) -> ExofsResult<()> {
        let entry = DedupEntry::new(blob_id, offset, size);
        let mut g = self.index.lock();
        // Vérifier que le blob n'existe pas déjà (double registration).
        if g.lookup(&blob_id).is_some() {
            g.inc_ref(&blob_id);
            return Ok(());
        }
        g.insert(entry)
    }

    /// Incrémente le compteur de références d'un blob.
    pub fn inc_ref(&self, blob_id: &BlobId) -> bool {
        self.index.lock().inc_ref(blob_id)
    }

    /// Décrémente le compteur de références.
    /// Retourne `Some(remaining)` ou `None` si non trouvé.
    pub fn dec_ref(&self, blob_id: &BlobId) -> Option<u32> {
        let rc = self.index.lock().dec_ref(blob_id)?;
        if rc == 0 {
            self.index.lock().remove(blob_id);
        }
        Some(rc)
    }

    /// Supprime un blob de l'index (gc).
    pub fn unregister(&self, blob_id: &BlobId) -> Option<DedupEntry> {
        self.index.lock().remove(blob_id)
    }

    pub fn hit_count(&self)    -> u64 { self.hits.load(Ordering::Relaxed) }
    pub fn miss_count(&self)   -> u64 { self.misses.load(Ordering::Relaxed) }
    pub fn bytes_saved(&self)  -> u64 { self.saved.load(Ordering::Relaxed) }
    pub fn entry_count(&self)  -> usize { self.index.lock().total_entries() }
    pub fn load_factor_pct(&self) -> u32 { self.index.lock().load_factor_pct() }

    pub fn dedup_ratio_milli(&self) -> u64 {
        let h = self.hit_count();
        let m = self.miss_count();
        let t = h.saturating_add(m);
        if t == 0 { 0 } else { h * 1000 / t }
    }
}

impl Default for DedupWriter {
    fn default() -> Self { Self::new() }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_offset(n: u64) -> DiskOffset { DiskOffset(n * 4096) }

    #[test]
    fn test_first_write_is_miss() {
        let w   = DedupWriter::new();
        let dec = w.check(b"unique data abc");
        assert!(dec.is_miss());
        assert_eq!(w.miss_count(), 1);
    }

    #[test]
    fn test_second_write_is_hit() {
        let w    = DedupWriter::new();
        let data = b"duplicate blob content 1234567890";
        let dec  = w.check(data);
        let id   = *dec.blob_id();
        w.register(id, make_offset(1), data.len() as u64).unwrap();

        let dec2 = w.check(data);
        assert!(dec2.is_hit());
        assert_eq!(w.hit_count(), 1);
    }

    #[test]
    fn test_bytes_saved() {
        let w    = DedupWriter::new();
        let data = b"some data ABCDEF0123456";
        let dec  = w.check(data);
        let id   = *dec.blob_id();
        w.register(id, make_offset(2), data.len() as u64).unwrap();
        w.check(data);
        assert_eq!(w.bytes_saved(), data.len() as u64);
    }

    #[test]
    fn test_entry_count_and_unregister() {
        let w    = DedupWriter::new();
        let data = b"content to dedup";
        let dec  = w.check(data);
        let id   = *dec.blob_id();
        w.register(id, make_offset(3), data.len() as u64).unwrap();
        assert_eq!(w.entry_count(), 1);
        w.unregister(&id);
        assert_eq!(w.entry_count(), 0);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DedupGarbageCollector — purge des entrées sans référence
// ─────────────────────────────────────────────────────────────────────────────

pub struct GcReport {
    pub entries_scanned:  u64,
    pub entries_removed:  u64,
    pub bytes_reclaimed:  u64,
}

impl DedupWriter {
    /// Retire toutes les entrées dont le ref_count est 0.
    pub fn gc(&self) -> ExofsResult<GcReport> {
        let mut idx     = self.index.lock();
        let mut removed = 0u64;
        let mut reclaimed = 0u64;
        let mut scanned = 0u64;

        for bucket in idx.buckets.iter_mut() {
            let before = bucket.len();
            bucket.entries.retain(|e| {
                scanned += 1;
                if e.ref_count == 0 {
                    reclaimed += e.size;
                    false
                } else {
                    true
                }
            });
            let after = bucket.len();
            removed += (before - after) as u64;
        }

        idx.total = idx.total.saturating_sub(removed as usize);

        Ok(GcReport { entries_scanned: scanned, entries_removed: removed, bytes_reclaimed: reclaimed })
    }

    /// Liste tous les blobs dont le ref_count est exactement 1 (candidats GC).
    pub fn orphans(&self) -> Vec<BlobId> {
        let idx = self.index.lock();
        let mut result = Vec::new();
        for bucket in &idx.buckets {
            for e in &bucket.entries {
                if e.ref_count == 1 {
                    let _ = result.try_reserve(1);
                    result.push(e.blob_id);
                }
            }
        }
        result
    }

    /// Exporte une snapshot de toutes les entrées pour sérialisation.
    pub fn snapshot(&self) -> ExofsResult<Vec<DedupEntry>> {
        let idx = self.index.lock();
        let total = idx.total;
        let mut snap: Vec<DedupEntry> = Vec::new();
        snap.try_reserve(total).map_err(|_| ExofsError::NoMemory)?;
        for bucket in &idx.buckets {
            for e in &bucket.entries {
                snap.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                snap.push(e.clone());
            }
        }
        Ok(snap)
    }

    /// Restaure l'index depuis un snapshot.
    pub fn restore_snapshot(&self, snap: &[DedupEntry]) -> ExofsResult<()> {
        let mut idx = self.index.lock();
        for entry in snap {
            if idx.lookup(&entry.blob_id).is_none() {
                idx.insert(entry.clone())?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests_extra {
    use super::*;
    use crate::fs::exofs::core::blob_id::compute_blob_id;

    #[test]
    fn test_gc_removes_zero_ref() {
        let w    = DedupWriter::new();
        let data = b"gc test data 12345";
        let dec  = w.check(data);
        let id   = *dec.blob_id();
        w.register(id, DiskOffset(0), data.len() as u64).unwrap();
        // Décrémente à 0.
        w.dec_ref(&id);
        let report = w.gc().unwrap();
        assert_eq!(report.entries_removed, 1);
    }

    #[test]
    fn test_snapshot_restore() {
        let w1   = DedupWriter::new();
        let data = b"snapshot content ABCDEF";
        let dec  = w1.check(data);
        let id   = *dec.blob_id();
        w1.register(id, DiskOffset(8192), data.len() as u64).unwrap();

        let snap = w1.snapshot().unwrap();
        let w2   = DedupWriter::new();
        w2.restore_snapshot(&snap).unwrap();
        assert_eq!(w2.entry_count(), 1);
    }

    #[test]
    fn test_orphan_detection() {
        let w   = DedupWriter::new();
        let d1  = b"orphan candidate data 1234";
        let dec = w.check(d1);
        w.register(*dec.blob_id(), DiskOffset(0), d1.len() as u64).unwrap();
        let orphans = w.orphans();
        assert!(!orphans.is_empty());
    }
}
