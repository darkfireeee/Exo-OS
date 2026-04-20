//! MetadataCache — cache des métadonnées d'inodes/entrées de répertoire (no_std).

//! metadata_cache.rs — Cache de métadonnées d'inodes ExoFS (no_std).
//!
//! `MetaEntry` : métadonnées complètes d'un inode mis en cache.
//! `MetadataCache` / `METADATA_CACHE` : cache global.
//! Règles : OOM-02, ARITH-02, RECUR-01.


extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::scheduler::sync::spinlock::SpinLock;
use super::cache_stats::CACHE_STATS;

// ── MetaKind ──────────────────────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetaKind {
    File      = 0,
    Directory = 1,
    Symlink   = 2,
    Device    = 3,
}

// ── MetaEntry ────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct MetaEntry {
    pub inode_id:    u64,
    pub size:        u64,
    pub mtime:       u64,
    pub kind:        MetaKind,
    pub flags:       u32,
    pub n_blobs:     u32,
    pub uid:         u32,
    pub gid:         u32,
    pub perm:        u16,
    pub link_count:  u16,
    /// Ticks de mise en cache.
    pub cached_tick: u64,
    pub dirty:       bool,
}

impl MetaEntry {
    pub fn new(inode_id: u64, size: u64, mtime: u64, kind: MetaKind,
               flags: u32, n_blobs: u32, uid: u32, gid: u32,
               perm: u16, link_count: u16, now: u64) -> Self {
        Self {
            inode_id, size, mtime, kind, flags, n_blobs,
            uid, gid, perm, link_count, cached_tick: now, dirty: false,
        }
    }

    pub fn is_stale(&self, now: u64, ttl: u64) -> bool {
        now.saturating_sub(self.cached_tick) >= ttl
    }
}

// ── MetadataCacheInner ────────────────────────────────────────────────────────

struct MetadataCacheInner {
    map: BTreeMap<u64, MetaEntry>,
    max: usize,
}

impl MetadataCacheInner {
    const fn new(max: usize) -> Self { Self { map: BTreeMap::new(), max } }

    fn evict_one(&mut self) {
        // LRU approxé : retire l'entrée avec le tick le plus bas.
        let oldest = self.map
            .iter()
            .min_by_key(|(_, v)| v.cached_tick)
            .map(|(k, _)| *k);
        if let Some(k) = oldest { self.map.remove(&k); }
    }
}

// ── MetadataCache ─────────────────────────────────────────────────────────────

pub struct MetadataCache {
    inner: SpinLock<MetadataCacheInner>,
}

pub static METADATA_CACHE: MetadataCache = MetadataCache::new_const();

impl MetadataCache {
    pub const fn new_const() -> Self {
        Self { inner: SpinLock::new(MetadataCacheInner::new(32768)) }
    }

    pub fn get(&self, inode_id: u64) -> Option<MetaEntry> {
        let inner = self.inner.lock();
        let r = inner.map.get(&inode_id).cloned();
        if r.is_some() { CACHE_STATS.record_hit(); }
        else           { CACHE_STATS.record_miss(); }
        r
    }

    pub fn insert(&self, mut meta: MetaEntry) -> ExofsResult<()> {
        let now = crate::arch::time::read_ticks();
        meta.cached_tick = now;
        let inode_id = meta.inode_id;
        let mut inner = self.inner.lock();
        if inner.map.len() >= inner.max { inner.evict_one(); }
        inner.map.insert(inode_id, meta);
        CACHE_STATS.record_insert(core::mem::size_of::<MetaEntry>() as u64);
        Ok(())
    }

    pub fn update<F>(&self, inode_id: u64, f: F) -> ExofsResult<()>
    where F: FnOnce(&mut MetaEntry)
    {
        let mut inner = self.inner.lock();
        match inner.map.get_mut(&inode_id) {
            Some(m) => { f(m); m.dirty = true; Ok(()) }
            None => Err(ExofsError::ObjectNotFound),
        }
    }

    pub fn invalidate(&self, inode_id: u64) {
        self.inner.lock().map.remove(&inode_id);
    }

    pub fn invalidate_batch(&self, ids: &[u64]) {
        let mut inner = self.inner.lock();
        for id in ids { inner.map.remove(id); }
    }

    pub fn mark_dirty(&self, inode_id: u64) -> ExofsResult<()> {
        let mut inner = self.inner.lock();
        match inner.map.get_mut(&inode_id) {
            Some(m) => { m.dirty = true; Ok(()) }
            None => Err(ExofsError::ObjectNotFound),
        }
    }

    pub fn dirty_ids(&self) -> Vec<u64> {
        self.inner.lock().map
            .iter()
            .filter(|(_, m)| m.dirty)
            .map(|(k, _)| *k)
            .collect()
    }

    pub fn evict_stale(&self, now: u64, ttl: u64) -> usize {
        let mut inner = self.inner.lock();
        let stale: Vec<u64> = inner.map
            .iter()
            .filter(|(_, m)| m.is_stale(now, ttl))
            .map(|(k, _)| *k)
            .collect();
        let count = stale.len();
        for id in stale { inner.map.remove(&id); }
        count
    }

    pub fn flush_all(&self) { self.inner.lock().map.clear(); }
    pub fn n_entries(&self) -> usize { self.inner.lock().map.len() }
}

// ── MetaCacheStats ─────────────────────────────────────────────────────────────

/// Statistiques spécifiques au cache de métadonnées.
pub struct MetaCacheStats {
    pub n_entries:      usize,
    pub n_dirty:        usize,
    pub n_files:        usize,
    pub n_directories:  usize,
    pub oldest_tick:    u64,
    pub newest_tick:    u64,
}

impl MetadataCache {
    /// Construit un rapport de statistiques détaillé.
    pub fn stats(&self) -> MetaCacheStats {
        let inner = self.inner.lock();
        let mut n_dirty = 0usize;
        let mut n_files = 0usize;
        let mut n_dirs  = 0usize;
        let mut oldest  = u64::MAX;
        let mut newest  = 0u64;
        for m in inner.map.values() {
            if m.dirty { n_dirty = n_dirty.wrapping_add(1); }
            match m.kind {
                MetaKind::File      => n_files = n_files.wrapping_add(1),
                MetaKind::Directory => n_dirs  = n_dirs.wrapping_add(1),
                _ => {}
            }
            if m.cached_tick < oldest { oldest = m.cached_tick; }
            if m.cached_tick > newest { newest = m.cached_tick; }
        }
        MetaCacheStats {
            n_entries:     inner.map.len(),
            n_dirty,
            n_files,
            n_directories: n_dirs,
            oldest_tick:   if oldest == u64::MAX { 0 } else { oldest },
            newest_tick:   newest,
        }
    }

    /// Insère un lot d'entrées. S'arrête au premier échec OOM.
    pub fn insert_batch(&self, metas: &[MetaEntry]) -> ExofsResult<usize> {
        let mut count = 0usize;
        for m in metas {
            self.insert(m.clone())?;
            count = count.wrapping_add(1);
        }
        Ok(count)
    }

    /// Met à jour la taille d'un inode.
    pub fn update_size(&self, inode_id: u64, new_size: u64) -> ExofsResult<()> {
        self.update(inode_id, |m| m.size = new_size)
    }

    /// Met à jour le mtime d'un inode.
    pub fn update_mtime(&self, inode_id: u64, mtime: u64) -> ExofsResult<()> {
        self.update(inode_id, |m| m.mtime = mtime)
    }

    /// Retourne tous les inodes d'un kind donné.
    pub fn by_kind(&self, kind: MetaKind) -> Vec<u64> {
        self.inner.lock().map
            .iter()
            .filter(|(_, m)| m.kind == kind)
            .map(|(k, _)| *k)
            .collect()
    }

    /// Invalide tous les inodes dirty.
    pub fn invalidate_all_dirty(&self) {
        let mut inner = self.inner.lock();
        let dirty: Vec<u64> = inner.map
            .iter()
            .filter(|(_, m)| m.dirty)
            .map(|(k, _)| *k)
            .collect();
        for id in dirty { inner.map.remove(&id); }
    }

    /// Nombre d'inodes dirty.
    pub fn n_dirty(&self) -> usize {
        self.inner.lock().map.values().filter(|m| m.dirty).count()
    }
}


// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(id: u64) -> MetaEntry {
        MetaEntry::new(id, 1024, 0, MetaKind::File, 0, 1, 0, 0, 0o644, 1, 0)
    }

    #[test] fn test_insert_get() {
        let c = MetadataCache::new_const();
        c.insert(mk(1)).unwrap();
        assert!(c.get(1).is_some());
    }

    #[test] fn test_miss() {
        let c = MetadataCache::new_const();
        assert!(c.get(99).is_none());
    }

    #[test] fn test_invalidate() {
        let c = MetadataCache::new_const();
        c.insert(mk(1)).unwrap();
        c.invalidate(1);
        assert!(c.get(1).is_none());
    }

    #[test] fn test_mark_dirty() {
        let c = MetadataCache::new_const();
        c.insert(mk(1)).unwrap();
        c.mark_dirty(1).unwrap();
        assert_eq!(c.dirty_ids().len(), 1);
    }

    #[test] fn test_mark_dirty_absent() {
        let c = MetadataCache::new_const();
        assert!(c.mark_dirty(99).is_err());
    }

    #[test] fn test_update() {
        let c = MetadataCache::new_const();
        c.insert(mk(1)).unwrap();
        c.update(1, |m| m.size = 9999).unwrap();
        assert_eq!(c.get(1).unwrap().size, 9999);
    }

    #[test] fn test_invalidate_batch() {
        let c = MetadataCache::new_const();
        c.insert(mk(1)).unwrap(); c.insert(mk(2)).unwrap(); c.insert(mk(3)).unwrap();
        c.invalidate_batch(&[1, 2]);
        assert_eq!(c.n_entries(), 1);
    }

    #[test] fn test_evict_stale() {
        let c = MetadataCache::new_const();
        let mut m = mk(1); m.cached_tick = 0;
        c.insert(m).unwrap();
        let evicted = c.evict_stale(10000, 100);
        assert_eq!(evicted, 1);
    }

    #[test] fn test_flush_all() {
        let c = MetadataCache::new_const();
        c.insert(mk(1)).unwrap(); c.insert(mk(2)).unwrap();
        c.flush_all();
        assert_eq!(c.n_entries(), 0);
    }

    #[test] fn test_by_kind_file() {
        let c = MetadataCache::new_const();
        c.insert(mk(1)).unwrap();
        c.insert(MetaEntry::new(2, 0, 0, MetaKind::Directory, 0, 0, 0, 0, 0, 0, 0)).unwrap();
        assert_eq!(c.by_kind(MetaKind::File).len(), 1);
        assert_eq!(c.by_kind(MetaKind::Directory).len(), 1);
    }

    #[test] fn test_insert_batch() {
        let c = MetadataCache::new_const();
        let batch: alloc::vec::Vec<MetaEntry> = (0..5).map(|i| mk(i)).collect();
        let n = c.insert_batch(&batch).unwrap();
        assert_eq!(n, 5);
        assert_eq!(c.n_entries(), 5);
    }

    #[test] fn test_update_size() {
        let c = MetadataCache::new_const();
        c.insert(mk(1)).unwrap();
        c.update_size(1, 9999).unwrap();
        assert_eq!(c.get(1).unwrap().size, 9999);
    }

    #[test] fn test_n_dirty() {
        let c = MetadataCache::new_const();
        c.insert(mk(1)).unwrap(); c.insert(mk(2)).unwrap();
        c.mark_dirty(1).unwrap();
        assert_eq!(c.n_dirty(), 1);
    }

    #[test] fn test_invalidate_all_dirty() {
        let c = MetadataCache::new_const();
        c.insert(mk(1)).unwrap(); c.insert(mk(2)).unwrap();
        c.mark_dirty(1).unwrap(); c.mark_dirty(2).unwrap();
        c.invalidate_all_dirty();
        assert_eq!(c.n_entries(), 0);
    }

    #[test] fn test_stats_report() {
        let c = MetadataCache::new_const();
        c.insert(mk(1)).unwrap();
        let s = c.stats();
        assert_eq!(s.n_entries, 1);
        assert_eq!(s.n_files, 1);
    }

    #[test] fn test_flush_all_empties_cache() {
        let c = MetadataCache::new_const();
        c.insert(mk(1)).unwrap();
        c.insert(mk(2)).unwrap();
        c.flush_all();
        assert_eq!(c.n_entries(), 0);
    }

    #[test] fn test_get_miss() {
        let c = MetadataCache::new_const();
        assert!(c.get(999).is_none());
    }

    #[test] fn test_remove_existing() {
        let c = MetadataCache::new_const();
        c.insert(mk(1)).unwrap();
        c.invalidate(1);
        assert!(c.get(1).is_none());
    }

    #[test] fn test_n_entries_increments() {
        let c = MetadataCache::new_const();
        c.insert(mk(3)).unwrap();
        c.insert(mk(4)).unwrap();
        assert_eq!(c.n_entries(), 2);
    }

    #[test] fn test_mark_dirty_then_not_dirty_after_flush() {
        let c = MetadataCache::new_const();
        c.insert(mk(5)).unwrap();
        c.mark_dirty(5).unwrap();
        c.invalidate_all_dirty();
        assert!(c.get(5).is_none());
    }

    #[test] fn test_stats_dirty_count() {
        let c = MetadataCache::new_const();
        c.insert(mk(6)).unwrap();
        c.mark_dirty(6).unwrap();
        let s = c.stats();
        assert!(s.n_dirty >= 1);
    }
}
