//! blob_cache.rs — Cache de blobs bruts ExoFS (no_std).
//!
//! `BlobCache` : cache LRU/ARC de données blob indexées par `BlobId`.
//! `BLOB_CACHE`  : instance globale statique.
//! Règles : OOM-02, ARITH-02, RECUR-01.

extern crate alloc;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;

use core::sync::atomic::{AtomicU64, Ordering};

use super::cache_eviction::{EvictionAlgorithm, EvictionPolicy};
use super::cache_stats::CACHE_STATS;
use crate::fs::exofs::core::{BlobId, ExofsError, ExofsResult};
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

const BLOB_CACHE_MAX_BYTES: u64 = 256 * 1024 * 1024;
const BLOB_ALLOC_PAGE_BYTES: usize = 4096;
const ARC_HEADER_BYTES: usize = core::mem::size_of::<usize>() * 2;
const BLOB_PAGE_SIZE: usize = BLOB_ALLOC_PAGE_BYTES - ARC_HEADER_BYTES;

// ─────────────────────────────────────────────────────────────────────────────
// BlobEntry
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée dans le cache de blobs.
struct BlobEntry {
    /// Données du blob, découpées en pages dont l'allocation totale avec
    /// l'en-tête Arc tient dans 4 KiB.
    pages: Vec<Option<Arc<[u8; BLOB_PAGE_SIZE]>>>,
    /// Snapshot contigu partagé pour les lectures complètes via `get()`.
    snapshot: Option<Arc<[u8]>>,
    /// Taille logique du blob.
    len: usize,
    /// `true` si l'entrée a été modifiée et n'a pas encore été écrite sur disque.
    dirty: bool,
    /// Ticks d'insertion.
    #[allow(dead_code)]
    inserted_at: u64,
    /// Ticks du dernier accès.
    last_accessed: u64,
    /// Nombre d'accès.
    access_count: u64,
}

impl BlobEntry {
    fn new(data: Vec<u8>, now: u64) -> ExofsResult<Self> {
        let len = data.len();
        let mut entry = Self {
            pages: Vec::new(),
            snapshot: None,
            len,
            dirty: false,
            inserted_at: now,
            last_accessed: now,
            access_count: 1,
        };
        if len != 0 {
            let page_count = len.saturating_add(BLOB_PAGE_SIZE - 1) / BLOB_PAGE_SIZE;
            entry
                .pages
                .try_reserve(page_count)
                .map_err(|_| ExofsError::NoMemory)?;
            let mut offset = 0usize;
            while offset < len {
                let n = (len - offset).min(BLOB_PAGE_SIZE);
                entry
                    .pages
                    .push(Self::page_from_slice(&data[offset..offset + n])?);
                offset = offset.wrapping_add(n);
            }
        }
        Ok(entry)
    }

    fn is_zero_slice(data: &[u8]) -> bool {
        let mut i = 0usize;
        while i < data.len() {
            if data[i] != 0 {
                return false;
            }
            i = i.wrapping_add(1);
        }
        true
    }

    fn page_from_slice(data: &[u8]) -> ExofsResult<Option<Arc<[u8; BLOB_PAGE_SIZE]>>> {
        if Self::is_zero_slice(data) {
            return Ok(None);
        }
        let mut page = Arc::new([0u8; BLOB_PAGE_SIZE]);
        Arc::get_mut(&mut page).ok_or(ExofsError::InternalError)?[..data.len()]
            .copy_from_slice(data);
        Ok(Some(page))
    }

    fn touch(&mut self, now: u64) {
        self.last_accessed = now;
        self.access_count = self.access_count.wrapping_add(1);
    }

    fn len(&self) -> u64 {
        self.len as u64
    }

    fn to_vec(&self) -> ExofsResult<Vec<u8>> {
        let mut out = Vec::new();
        out.try_reserve(self.len)
            .map_err(|_| ExofsError::NoMemory)?;
        let mut copied = 0usize;
        while copied < self.len {
            let page_idx = copied / BLOB_PAGE_SIZE;
            let page_off = copied % BLOB_PAGE_SIZE;
            let n = (self.len - copied).min(BLOB_PAGE_SIZE - page_off);
            if let Some(page) = &self.pages[page_idx] {
                out.extend_from_slice(&page[page_off..page_off + n]);
            } else {
                let new_len = out.len().saturating_add(n);
                out.resize(new_len, 0);
            }
            copied = copied.wrapping_add(n);
        }
        Ok(out)
    }

    fn materialize_snapshot(&mut self) -> ExofsResult<Arc<[u8]>> {
        if let Some(snapshot) = &self.snapshot {
            return Ok(Arc::clone(snapshot));
        }
        let data = self.to_vec()?;
        let snapshot: Arc<[u8]> = Arc::from(data.into_boxed_slice());
        self.snapshot = Some(Arc::clone(&snapshot));
        Ok(snapshot)
    }

    fn read_range(&self, offset: usize, count: usize) -> ExofsResult<Vec<u8>> {
        if count == 0 || offset >= self.len {
            return Ok(Vec::new());
        }
        let read_len = count.min(self.len - offset);
        let mut out = Vec::new();
        out.try_reserve(read_len)
            .map_err(|_| ExofsError::NoMemory)?;

        let mut copied = 0usize;
        while copied < read_len {
            let pos = offset + copied;
            let page_idx = pos / BLOB_PAGE_SIZE;
            let page_off = pos % BLOB_PAGE_SIZE;
            let n = (read_len - copied).min(BLOB_PAGE_SIZE - page_off);
            if let Some(page) = &self.pages[page_idx] {
                out.extend_from_slice(&page[page_off..page_off + n]);
            } else {
                let new_len = out.len().saturating_add(n);
                out.resize(new_len, 0);
            }
            copied = copied.wrapping_add(n);
        }
        Ok(out)
    }

    fn ensure_page(&mut self, page_idx: usize) -> ExofsResult<()> {
        while self.pages.len() <= page_idx {
            self.pages.push(None);
        }
        Ok(())
    }

    fn write_range(&mut self, offset: usize, bytes: &[u8]) -> ExofsResult<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let end = offset.checked_add(bytes.len()).ok_or(ExofsError::NoSpace)?;
        let last_page = (end - 1) / BLOB_PAGE_SIZE;
        self.ensure_page(last_page)?;

        let mut copied = 0usize;
        while copied < bytes.len() {
            let pos = offset + copied;
            let page_idx = pos / BLOB_PAGE_SIZE;
            let page_off = pos % BLOB_PAGE_SIZE;
            let n = (bytes.len() - copied).min(BLOB_PAGE_SIZE - page_off);
            let chunk = &bytes[copied..copied + n];
            if Self::is_zero_slice(chunk) && self.pages[page_idx].is_none() {
                copied = copied.wrapping_add(n);
                continue;
            }

            let mut page = Arc::new([0u8; BLOB_PAGE_SIZE]);
            let page_mut = Arc::get_mut(&mut page).ok_or(ExofsError::InternalError)?;
            if let Some(existing) = &self.pages[page_idx] {
                page_mut.copy_from_slice(&existing[..]);
            }
            page_mut[page_off..page_off + n].copy_from_slice(chunk);
            self.pages[page_idx] = if Self::is_zero_slice(page_mut) {
                None
            } else {
                Some(page)
            };
            copied = copied.wrapping_add(n);
        }
        self.len = self.len.max(end);
        self.snapshot = None;
        Ok(())
    }

    fn resize(&mut self, new_len: usize) -> ExofsResult<()> {
        if new_len == 0 {
            self.pages.clear();
            self.snapshot = None;
            self.len = 0;
            return Ok(());
        }

        if new_len > self.len {
            let last_page = (new_len - 1) / BLOB_PAGE_SIZE;
            self.ensure_page(last_page)?;
        } else {
            let keep_pages = new_len.saturating_add(BLOB_PAGE_SIZE - 1) / BLOB_PAGE_SIZE;
            self.pages.truncate(keep_pages);
            let tail = new_len % BLOB_PAGE_SIZE;
            if tail != 0 && !self.pages.is_empty() {
                let page_idx = self.pages.len() - 1;
                if self.pages[page_idx].is_none() {
                    self.len = new_len;
                    self.snapshot = None;
                    return Ok(());
                }
                let mut page = Arc::new([0u8; BLOB_PAGE_SIZE]);
                let page_mut = Arc::get_mut(&mut page).ok_or(ExofsError::InternalError)?;
                if let Some(existing) = &self.pages[page_idx] {
                    page_mut.copy_from_slice(&existing[..]);
                }
                page_mut[tail..].fill(0);
                self.pages[page_idx] = if Self::is_zero_slice(page_mut) {
                    None
                } else {
                    Some(page)
                };
            }
        }

        self.len = new_len;
        self.snapshot = None;
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BlobCacheInner
// ─────────────────────────────────────────────────────────────────────────────

struct BlobCacheInner {
    map: BTreeMap<BlobId, BlobEntry>,
    eviction: EvictionPolicy,
    used: u64,
}

impl BlobCacheInner {
    const fn new() -> Self {
        Self {
            map: BTreeMap::new(),
            eviction: EvictionPolicy::new(EvictionAlgorithm::Arc),
            used: 0,
        }
    }

    fn evict_to_fit(&mut self, needed: u64, max_bytes: u64) -> ExofsResult<()> {
        self.evict_to_fit_except(needed, max_bytes, None)
    }

    fn evict_to_fit_except(
        &mut self,
        needed: u64,
        max_bytes: u64,
        protected: Option<BlobId>,
    ) -> ExofsResult<()> {
        let mut iters: usize = 0;
        while self.used.saturating_add(needed) > max_bytes {
            let victims = self.eviction.pick_eviction_candidates(4);
            if victims.is_empty() {
                return Err(ExofsError::NoSpace);
            }
            let mut removed_any = false;
            for v in &victims {
                if protected == Some(*v) {
                    continue;
                }
                if let Some(e) = self.map.remove(v) {
                    let sz = e.len();
                    self.eviction.remove(v);
                    self.used = self.used.saturating_sub(sz);
                    CACHE_STATS.record_eviction(sz);
                    removed_any = true;
                }
            }
            if !removed_any {
                return Err(ExofsError::NoSpace);
            }
            iters = iters.wrapping_add(1);
            if iters > 64 {
                return Err(ExofsError::NoSpace);
            }
        }
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BlobCache
// ─────────────────────────────────────────────────────────────────────────────

/// Cache de blobs bruts avec éviction et statistiques.
pub struct BlobCache {
    inner: SpinLock<BlobCacheInner>,
    hits: AtomicU64,
    misses: AtomicU64,
    max_bytes: u64,
}

pub static BLOB_CACHE: BlobCache = BlobCache::new_const();

impl BlobCache {
    pub const fn new_const() -> Self {
        Self {
            inner: SpinLock::new(BlobCacheInner::new()),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            max_bytes: BLOB_CACHE_MAX_BYTES,
        }
    }

    // ── Lecture ──────────────────────────────────────────────────────────────

    /// Retourne un snapshot partagé des données du blob, ou `None` si absent.
    pub fn get(&self, id: &BlobId) -> Option<Arc<[u8]>> {
        let mut inner = self.inner.lock();
        let now = crate::arch::time::read_ticks();
        let snapshot = {
            let entry = match inner.map.get_mut(id) {
                Some(entry) => entry,
                None => {
                    self.misses.fetch_add(1, Ordering::Relaxed);
                    CACHE_STATS.record_miss();
                    return None;
                }
            };
            entry.touch(now);
            match entry.materialize_snapshot() {
                Ok(data) => data,
                Err(_) => return None,
            }
        };

        inner.eviction.touch(id);
        self.hits.fetch_add(1, Ordering::Relaxed);
        CACHE_STATS.record_hit();
        Some(snapshot)
    }

    /// Retourne la taille d'un blob sans cloner son contenu.
    pub fn len(&self, id: &BlobId) -> Option<usize> {
        self.inner.lock().map.get(id).map(|e| e.len)
    }

    /// Copie uniquement l'intervalle demandé d'un blob.
    pub fn read_at(&self, id: &BlobId, offset: usize, count: usize) -> ExofsResult<Vec<u8>> {
        if count == 0 {
            return Ok(Vec::new());
        }

        let mut inner = self.inner.lock();
        let now = crate::arch::time::read_ticks();
        let out = {
            let entry = match inner.map.get_mut(id) {
                Some(entry) => entry,
                None => {
                    self.misses.fetch_add(1, Ordering::Relaxed);
                    CACHE_STATS.record_miss();
                    return Err(ExofsError::BlobNotFound);
                }
            };

            entry.touch(now);
            entry.read_range(offset, count)?
        };

        inner.eviction.touch(id);
        self.hits.fetch_add(1, Ordering::Relaxed);
        CACHE_STATS.record_hit();
        Ok(out)
    }

    /// `true` si le blob est présent en cache.
    pub fn contains(&self, id: &BlobId) -> bool {
        self.inner.lock().map.contains_key(id)
    }

    // ── Écriture ─────────────────────────────────────────────────────────────

    /// Insère ou met à jour un blob dans le cache.
    pub fn insert(&self, id: BlobId, data: Vec<u8>) -> ExofsResult<()> {
        let size = data.len() as u64;
        let now = crate::arch::time::read_ticks();
        let max = self.max_bytes;
        let mut inner = self.inner.lock();

        // Si déjà présent, mettre à jour.
        if inner.map.contains_key(&id) {
            let old_size = inner.map[&id].len();
            inner.used = inner.used.saturating_sub(old_size);
            inner.eviction.remove(&id);
            let existing = inner.map.get_mut(&id).ok_or(ExofsError::InternalError)?;
            *existing = BlobEntry::new(data, now)?;
            existing.dirty = false;
            existing.touch(now);
            inner.used = inner.used.saturating_add(size);
            inner.eviction.insert(id, size)?;
            CACHE_STATS.record_insert(size);
            return Ok(());
        }

        // OOM-02 : réserver avant insertion.
        inner.evict_to_fit(size, max)?;

        let entry = BlobEntry::new(data, now)?;
        inner.map.insert(id, entry);
        inner.eviction.insert(id, size)?;
        inner.used = inner.used.saturating_add(size);
        CACHE_STATS.record_insert(size);
        Ok(())
    }

    /// Ecrit un intervalle dans un blob sans cloner tout le blob existant.
    pub fn write_at(&self, id: BlobId, offset: usize, bytes: &[u8]) -> ExofsResult<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }

        let end = offset.checked_add(bytes.len()).ok_or(ExofsError::NoSpace)?;
        let now = crate::arch::time::read_ticks();
        let max = self.max_bytes;
        let mut inner = self.inner.lock();

        if inner.map.contains_key(&id) {
            let old_size = inner
                .map
                .get(&id)
                .map(|e| e.len())
                .ok_or(ExofsError::InternalError)?;
            let new_size = (end as u64).max(old_size);
            let growth = new_size.saturating_sub(old_size);
            if growth > 0 {
                inner.evict_to_fit_except(growth, max, Some(id))?;
            }

            inner.eviction.remove(&id);
            let was_dirty;
            {
                let entry = inner.map.get_mut(&id).ok_or(ExofsError::InternalError)?;
                was_dirty = entry.dirty;
                entry.write_range(offset, bytes)?;
                entry.dirty = true;
                entry.touch(now);
            }

            let final_size = inner
                .map
                .get(&id)
                .map(|e| e.len() as u64)
                .ok_or(ExofsError::InternalError)?;
            inner.used = inner
                .used
                .saturating_sub(old_size)
                .saturating_add(final_size);
            inner.eviction.insert(id, final_size)?;
            let dirty_delta = if was_dirty {
                final_size.saturating_sub(old_size)
            } else {
                final_size
            };
            if dirty_delta > 0 {
                CACHE_STATS.record_dirty_add(dirty_delta);
            }
            CACHE_STATS.record_insert(final_size);
            return Ok(bytes.len());
        }

        let size = end as u64;

        inner.evict_to_fit(size, max)?;
        let mut entry = BlobEntry::new(Vec::new(), now)?;
        entry.write_range(offset, bytes)?;
        entry.dirty = true;
        inner.map.insert(id, entry);
        inner.eviction.insert(id, size)?;
        inner.used = inner.used.saturating_add(size);
        CACHE_STATS.record_insert(size);
        CACHE_STATS.record_dirty_add(size);
        Ok(bytes.len())
    }

    /// Redimensionne un blob sans materialiser son contenu dans un bloc contigu.
    pub fn resize(&self, id: BlobId, new_len: usize) -> ExofsResult<()> {
        let now = crate::arch::time::read_ticks();
        let max = self.max_bytes;
        let mut inner = self.inner.lock();

        let old_size = inner
            .map
            .get(&id)
            .map(|entry| entry.len())
            .ok_or(ExofsError::BlobNotFound)?;
        let new_size = new_len as u64;
        let growth = new_size.saturating_sub(old_size);
        if growth > 0 {
            inner.evict_to_fit_except(growth, max, Some(id))?;
        }

        inner.eviction.remove(&id);
        let was_dirty;
        {
            let entry = inner.map.get_mut(&id).ok_or(ExofsError::BlobNotFound)?;
            was_dirty = entry.dirty;
            entry.resize(new_len)?;
            entry.dirty = true;
            entry.touch(now);
        }

        inner.used = inner.used.saturating_sub(old_size).saturating_add(new_size);
        inner.eviction.insert(id, new_size)?;
        let dirty_delta = if was_dirty {
            new_size.saturating_sub(old_size)
        } else {
            new_size
        };
        if dirty_delta > 0 {
            CACHE_STATS.record_dirty_add(dirty_delta);
        }
        CACHE_STATS.record_insert(new_size);
        Ok(())
    }

    /// Invalide (supprime) une entrée du cache.
    pub fn invalidate(&self, id: &BlobId) {
        let mut inner = self.inner.lock();
        if let Some(e) = inner.map.remove(id) {
            let sz = e.len();
            inner.eviction.remove(id);
            inner.used = inner.used.saturating_sub(sz);
            CACHE_STATS.record_invalidation(sz);
        }
    }

    /// Marque une entrée comme dirty (non synchronisée).
    pub fn mark_dirty(&self, id: &BlobId) -> ExofsResult<()> {
        let mut inner = self.inner.lock();
        match inner.map.get_mut(id) {
            Some(e) => {
                if !e.dirty {
                    e.dirty = true;
                    CACHE_STATS.record_dirty_add(e.len());
                }
                Ok(())
            }
            None => Err(ExofsError::ObjectNotFound),
        }
    }

    /// Retourne les IDs de toutes les entrées dirty.
    pub fn dirty_ids(&self) -> Vec<BlobId> {
        let inner = self.inner.lock();
        inner
            .map
            .iter()
            .filter(|(_, e)| e.dirty)
            .map(|(k, _)| *k)
            .collect()
    }

    /// Marque une entrée comme propre (après flush).
    pub fn mark_clean(&self, id: &BlobId) -> ExofsResult<()> {
        let mut inner = self.inner.lock();
        match inner.map.get_mut(id) {
            Some(e) => {
                if e.dirty {
                    let sz = e.len();
                    e.dirty = false;
                    CACHE_STATS.record_dirty_flush(sz);
                }
                Ok(())
            }
            None => Err(ExofsError::ObjectNotFound),
        }
    }

    // ── Statistiques ──────────────────────────────────────────────────────────

    pub fn used_bytes(&self) -> u64 {
        self.inner.lock().used
    }

    pub fn n_entries(&self) -> usize {
        self.inner.lock().map.len()
    }

    /// Retourne la liste de tous les `BlobId` présents dans le cache.
    pub fn list_keys(&self) -> ExofsResult<Vec<BlobId>> {
        let inner = self.inner.lock();
        let mut keys = Vec::new();
        keys.try_reserve(inner.map.len())
            .map_err(|_| ExofsError::NoMemory)?;
        for k in inner.map.keys() {
            keys.push(*k);
        }
        Ok(keys)
    }

    pub fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }
    pub fn misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }
    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    pub fn hit_ratio_pct(&self) -> u64 {
        let h = self.hits();
        let m = self.misses();
        let t = h.wrapping_add(m);
        if t == 0 {
            0
        } else {
            h * 100 / t
        }
    }

    /// Évince `n` entrées candidates (les plus froides).
    pub fn evict_n(&self, n: usize) -> u64 {
        let mut inner = self.inner.lock();
        let victims = inner.eviction.pick_eviction_candidates(n);
        let mut freed = 0u64;
        for id in &victims {
            if let Some(e) = inner.map.remove(id) {
                let sz = e.len();
                inner.eviction.remove(id);
                inner.used = inner.used.saturating_sub(sz);
                freed = freed.saturating_add(sz);
                CACHE_STATS.record_eviction(sz);
            }
        }
        freed
    }

    /// Vide entièrement le cache après avoir vérifié qu'il n'y a pas d'entrées dirty.
    ///
    /// # Erreur
    /// Retourne `Err(ExofsError::DirtyDataLoss(n))` si `n` entrées dirty
    /// seraient perdues. Appeler `flush_dirty_to_disk()` avant, ou utiliser
    /// `flush_all_force()` uniquement en contexte de panique/arrêt d'urgence.
    pub fn flush_all(&self) -> ExofsResult<()> {
        let mut inner = self.inner.lock();
        let dirty_count = inner.map.values().filter(|e| e.dirty).count();
        if dirty_count > 0 {
            return Err(ExofsError::DirtyDataLoss(dirty_count));
        }
        inner.map.clear();
        inner.used = 0;
        Ok(())
    }

    /// Vide le cache sans vérification — UNIQUEMENT pour panic/shutdown d'urgence.
    ///
    /// # Safety sémantique
    /// Les entrées dirty sont perdues sans écriture disque.
    /// Ne pas appeler en dehors d'un contexte d'arrêt non-récupérable.
    pub fn flush_all_force(&self) {
        let mut inner = self.inner.lock();
        let lost: u64 = inner
            .map
            .values()
            .filter(|e| e.dirty)
            .map(|e| e.len())
            .sum();
        if lost > 0 {
            CACHE_STATS.record_eviction(lost);
        }
        inner.map.clear();
        inner.used = 0;
    }

    /// Retourne les données de toutes les entrées dirty pour écriture disque.
    ///
    /// Le appelant est responsable d'appeler `mark_clean()` après écriture réussie.
    pub fn collect_dirty(&self) -> Vec<(BlobId, Box<[u8]>)> {
        let inner = self.inner.lock();
        inner
            .map
            .iter()
            .filter(|(_, e)| e.dirty)
            .filter_map(|(id, e)| e.to_vec().ok().map(|data| (*id, data.into_boxed_slice())))
            .collect()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;

    fn blob(b: u8) -> BlobId {
        BlobId([b; 32])
    }

    #[test]
    fn test_insert_and_get() {
        let c = BlobCache::new_const();
        c.insert(blob(1), alloc::vec![0u8; 64]).test_unwrap();
        assert!(c.get(&blob(1)).is_some());
    }

    #[test]
    fn test_page_payload_keeps_arc_allocation_within_4k() {
        assert_eq!(BLOB_PAGE_SIZE + ARC_HEADER_BYTES, BLOB_ALLOC_PAGE_BYTES);
    }

    #[test]
    fn test_miss_increments_counter() {
        let c = BlobCache::new_const();
        assert!(c.get(&blob(42)).is_none());
        assert_eq!(c.misses(), 1);
    }

    #[test]
    fn test_hit_increments_counter() {
        let c = BlobCache::new_const();
        c.insert(blob(1), alloc::vec![0u8; 32]).test_unwrap();
        c.get(&blob(1));
        assert_eq!(c.hits(), 1);
    }

    #[test]
    fn test_get_reuses_arc_snapshot() {
        let c = BlobCache::new_const();
        c.insert(blob(11), b"snapshot".to_vec()).test_unwrap();
        let first = c.get(&blob(11)).test_unwrap();
        let second = c.get(&blob(11)).test_unwrap();
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn test_write_at_invalidates_arc_snapshot() {
        let c = BlobCache::new_const();
        c.insert(blob(12), b"abcdef".to_vec()).test_unwrap();
        let before = c.get(&blob(12)).test_unwrap();
        c.write_at(blob(12), 2, b"XY").test_unwrap();
        let after = c.get(&blob(12)).test_unwrap();
        assert!(!Arc::ptr_eq(&before, &after));
        assert_eq!(&after[..], b"abXYef");
    }

    #[test]
    fn test_invalidate_removes_entry() {
        let c = BlobCache::new_const();
        c.insert(blob(1), alloc::vec![0u8; 32]).test_unwrap();
        c.invalidate(&blob(1));
        assert!(c.get(&blob(1)).is_none());
    }

    #[test]
    fn test_mark_dirty_and_clean() {
        let c = BlobCache::new_const();
        c.insert(blob(1), alloc::vec![0u8; 32]).test_unwrap();
        c.mark_dirty(&blob(1)).test_unwrap();
        assert_eq!(c.dirty_ids().len(), 1);
        c.mark_clean(&blob(1)).test_unwrap();
        assert_eq!(c.dirty_ids().len(), 0);
    }

    #[test]
    fn test_mark_dirty_absent_returns_err() {
        let c = BlobCache::new_const();
        assert!(c.mark_dirty(&blob(99)).is_err());
    }

    #[test]
    fn test_used_bytes_tracks_insertions() {
        let c = BlobCache::new_const();
        c.insert(blob(1), alloc::vec![0u8; 128]).test_unwrap();
        assert_eq!(c.used_bytes(), 128);
    }

    #[test]
    fn test_used_bytes_decreases_on_invalidate() {
        let c = BlobCache::new_const();
        c.insert(blob(1), alloc::vec![0u8; 128]).test_unwrap();
        c.invalidate(&blob(1));
        assert_eq!(c.used_bytes(), 0);
    }

    #[test]
    fn test_contains() {
        let c = BlobCache::new_const();
        assert!(!c.contains(&blob(5)));
        c.insert(blob(5), alloc::vec![0u8; 8]).test_unwrap();
        assert!(c.contains(&blob(5)));
    }

    #[test]
    fn test_len_does_not_clone_blob() {
        let c = BlobCache::new_const();
        c.insert(blob(6), alloc::vec![0u8; 128]).test_unwrap();
        assert_eq!(c.len(&blob(6)), Some(128));
        assert_eq!(c.hits(), 0);
    }

    #[test]
    fn test_read_at_returns_only_requested_range() {
        let c = BlobCache::new_const();
        c.insert(blob(7), b"abcdef".to_vec()).test_unwrap();
        let out = c.read_at(&blob(7), 2, 3).test_unwrap();
        assert_eq!(&out[..], b"cde");
    }

    #[test]
    fn test_write_at_updates_existing_blob_in_place() {
        let c = BlobCache::new_const();
        c.insert(blob(8), b"abcdef".to_vec()).test_unwrap();
        c.write_at(blob(8), 2, b"XY").test_unwrap();
        let out = c.get(&blob(8)).test_unwrap();
        assert_eq!(&out[..], b"abXYef");
        assert_eq!(c.used_bytes(), 6);
    }

    #[test]
    fn test_write_at_extends_sparse_blob() {
        let c = BlobCache::new_const();
        c.write_at(blob(9), 4, b"xy").test_unwrap();
        let out = c.get(&blob(9)).test_unwrap();
        assert_eq!(&out[..], b"\0\0\0\0xy");
        assert_eq!(c.used_bytes(), 6);
    }

    #[test]
    fn test_zero_writes_remain_sparse() {
        let c = BlobCache::new_const();
        let id = blob(13);
        let zeros = alloc::vec![0u8; 1024 * 1024];

        let mut idx = 0usize;
        while idx < 8 {
            c.write_at(id, idx * 1024 * 1024, &zeros).test_unwrap();
            idx = idx.wrapping_add(1);
        }

        assert_eq!(c.len(&id), Some(8 * 1024 * 1024));
        let inner = c.inner.lock();
        let entry = inner.map.get(&id).unwrap();
        assert!(entry.pages.iter().all(|page| page.is_none()));
    }

    #[test]
    fn test_resize_grow_remains_sparse() {
        let c = BlobCache::new_const();
        let id = blob(14);
        c.insert(id, Vec::new()).test_unwrap();
        c.resize(id, 128 * 1024 * 1024).test_unwrap();

        assert_eq!(c.len(&id), Some(128 * 1024 * 1024));
        let inner = c.inner.lock();
        let entry = inner.map.get(&id).unwrap();
        assert!(entry.pages.iter().all(|page| page.is_none()));
    }

    #[test]
    fn test_write_at_handles_16_mib_without_contiguous_blob() {
        let c = BlobCache::new_const();
        let id = blob(10);
        let block = alloc::vec![0x5Au8; 1024 * 1024];

        let mut idx = 0usize;
        while idx < 16 {
            c.write_at(id, idx * 1024 * 1024, &block).test_unwrap();
            idx += 1;
        }

        assert_eq!(c.len(&id), Some(16 * 1024 * 1024));
        let tail = c.read_at(&id, 15 * 1024 * 1024, 1024).test_unwrap();
        assert_eq!(tail.len(), 1024);
        assert!(tail.iter().all(|byte| *byte == 0x5A));
    }

    #[test]
    fn test_flush_all_clean_succeeds() {
        let c = BlobCache::new_const();
        c.insert(blob(1), alloc::vec![0u8; 64]).test_unwrap();
        c.flush_all().test_unwrap();
        assert_eq!(c.n_entries(), 0);
    }

    #[test]
    fn test_flush_all_dirty_returns_err() {
        let c = BlobCache::new_const();
        c.insert(blob(1), alloc::vec![0u8; 64]).test_unwrap();
        c.mark_dirty(&blob(1)).test_unwrap();
        assert!(c.flush_all().is_err());
        assert_eq!(c.n_entries(), 1);
    }

    #[test]
    fn test_flush_all_force_clears_dirty() {
        let c = BlobCache::new_const();
        c.insert(blob(1), alloc::vec![0u8; 64]).test_unwrap();
        c.mark_dirty(&blob(1)).test_unwrap();
        c.flush_all_force();
        assert_eq!(c.n_entries(), 0);
    }

    #[test]
    fn test_hit_ratio_pct() {
        let c = BlobCache::new_const();
        c.insert(blob(1), alloc::vec![0u8; 16]).test_unwrap();
        c.get(&blob(1));
        c.get(&blob(1));
        c.get(&blob(2));
        assert_eq!(c.hit_ratio_pct(), 66);
    }
}
