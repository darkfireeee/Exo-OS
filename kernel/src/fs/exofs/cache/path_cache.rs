//! PathCache — cache de résolution de chemins ExoFS (no_std).
//!
//! RÈGLE 10 : pas de buffers PATH_MAX sur la pile kernel.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::FsError;
use super::cache_stats::CACHE_STATS;

const PATH_CACHE_MAX: usize = 16384;

/// Résultat d'une résolution de chemin.
#[derive(Clone, Debug)]
pub struct PathResolution {
    pub path:     String,
    pub inode_id: u64,
    pub flags:    u32,
    pub tick:     u64,
}

pub static PATH_CACHE: PathCache = PathCache::new_const();

pub struct PathCache {
    map:   SpinLock<BTreeMap<u64, PathResolution>>,  // Clé = hash du chemin.
    hits:  AtomicU64,
    miss:  AtomicU64,
    inv:   AtomicU64,
}

impl PathCache {
    pub const fn new_const() -> Self {
        Self {
            map:  SpinLock::new(BTreeMap::new()),
            hits: AtomicU64::new(0),
            miss: AtomicU64::new(0),
            inv:  AtomicU64::new(0),
        }
    }

    fn path_hash(path: &[u8]) -> u64 {
        // FNV-1a 64-bit.
        let mut h: u64 = 0xcbf29ce484222325;
        for &b in path {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    pub fn lookup(&self, path: &[u8]) -> Option<PathResolution> {
        let key = Self::path_hash(path);
        let map = self.map.lock();
        if let Some(r) = map.get(&key) {
            // Vérification que le chemin correspond (anti-collision).
            if r.path.as_bytes() == path {
                self.hits.fetch_add(1, Ordering::Relaxed);
                CACHE_STATS.record_hit();
                return Some(r.clone());
            }
        }
        self.miss.fetch_add(1, Ordering::Relaxed);
        CACHE_STATS.record_miss();
        None
    }

    pub fn insert(&self, path: &[u8], inode_id: u64, flags: u32) -> Result<(), FsError> {
        let key = Self::path_hash(path);
        let tick = crate::arch::time::read_ticks();
        let path_str = {
            let mut s = String::new();
            // RÈGLE 10 : chemin sur heap (String alloué), jamais sur stack.
            s.try_reserve(path.len()).map_err(|_| FsError::OutOfMemory)?;
            for &b in path {
                s.push(b as char);
            }
            s
        };

        let mut map = self.map.lock();
        if map.len() >= PATH_CACHE_MAX {
            if let Some(k) = map.keys().next().copied() { map.remove(&k); }
        }
        map.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        map.insert(key, PathResolution { path: path_str, inode_id, flags, tick });
        Ok(())
    }

    /// Invalide toutes les entrées dont le chemin commence par `prefix`.
    pub fn invalidate_prefix(&self, prefix: &[u8]) {
        let mut map = self.map.lock();
        let to_remove: Vec<u64> = map
            .iter()
            .filter(|(_, r)| r.path.as_bytes().starts_with(prefix))
            .map(|(k, _)| *k)
            .collect();
        for k in to_remove {
            map.remove(&k);
            self.inv.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn invalidate_inode(&self, inode_id: u64) {
        let mut map = self.map.lock();
        map.retain(|_, r| {
            if r.inode_id == inode_id {
                self.inv.fetch_add(1, Ordering::Relaxed);
                false
            } else {
                true
            }
        });
    }

    pub fn size(&self) -> usize { self.map.lock().len() }
    pub fn hits(&self) -> u64 { self.hits.load(Ordering::Relaxed) }
    pub fn misses(&self) -> u64 { self.miss.load(Ordering::Relaxed) }
    pub fn invalidations(&self) -> u64 { self.inv.load(Ordering::Relaxed) }
}
