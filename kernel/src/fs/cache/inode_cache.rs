// kernel/src/fs/cache/inode_cache.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// INODE CACHE — Cache de métadonnées (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Cache hash-indexé des inodes chargés depuis le disque.
// Complète le `InodeCacheSimple` de core/inode.rs par une hash table
// à capacité réglable avec LRU-approximé (clock algorithm).
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fs::core::types::{InodeNumber, FsResult, FsError, FS_STATS};
use crate::fs::core::inode::InodeRef;
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

const ICACHE_BUCKETS: usize = 1024;
const ICACHE_MASK: usize    = ICACHE_BUCKETS - 1;

// ─────────────────────────────────────────────────────────────────────────────
// InodeCacheEntry
// ─────────────────────────────────────────────────────────────────────────────

struct InodeCacheEntry {
    /// ID superbloc + inode = clé composite.
    sb_id:      u32,
    ino:        InodeNumber,
    pub inode:      InodeRef,
    /// Tick du dernier accès (clock LRU).
    access_tick: u64,
    /// Bit de référence (clock algorithm).
    referenced:  bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// InodeHashCache — table de hachage
// ─────────────────────────────────────────────────────────────────────────────

struct InodeBucket {
    pub entries: SpinLock<Vec<InodeCacheEntry>>,
}

impl InodeBucket {
    const fn new() -> Self {
        Self { entries: SpinLock::new(Vec::new()) }
    }
}

/// Cache d'inodes hash-indexé avec clock LRU.
pub struct InodeHashCache {
    pub buckets:    [InodeBucket; ICACHE_BUCKETS],
    pub total:      AtomicUsize,
    cap:        usize,
    clock_tick: AtomicU64,
}

impl InodeHashCache {
    #[inline(always)]
    fn hash(&self, sb_id: u32, ino: InodeNumber) -> usize {
        let h = (sb_id as u64).wrapping_mul(0x9e3779b97f4a7c15)
            ^ ino.as_u64().wrapping_mul(0x6c62272e07bb0142);
        (h as usize) & ICACHE_MASK
    }

    /// Recherche un inode.
    pub fn lookup(&self, sb_id: u32, ino: InodeNumber) -> Option<InodeRef> {
        let idx = self.hash(sb_id, ino);
        let tick = self.clock_tick.fetch_add(1, Ordering::Relaxed);
        let mut entries = self.buckets[idx].entries.lock();
        for e in entries.iter_mut() {
            if e.sb_id == sb_id && e.ino == ino {
                e.access_tick = tick;
                e.referenced = true;
                FS_STATS.cache_hits.fetch_add(1, Ordering::Relaxed);
                return Some(e.inode.clone());
            }
        }
        FS_STATS.cache_misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Insère un inode. Évince si la capacité est atteinte.
    pub fn insert(&self, sb_id: u32, ino: InodeNumber, inode: InodeRef) {
        let idx = self.hash(sb_id, ino);
        let tick = self.clock_tick.fetch_add(1, Ordering::Relaxed);
        let mut entries = self.buckets[idx].entries.lock();

        // Remplacer existant.
        entries.retain(|e| !(e.sb_id == sb_id && e.ino == ino));

        // Évincer si plein.
        if self.cap > 0 && self.total.load(Ordering::Relaxed) >= self.cap {
            // Clock algorithm : cherche la première non-référencée.
            let evict_pos = entries.iter().position(|e| !e.referenced)
                .or_else(|| {
                    // Deuxième passe : tout marquer non référencé et reprendre.
                    for e in entries.iter_mut() { e.referenced = false; }
                    entries.iter().position(|e| !e.referenced)
                });
            if let Some(pos) = evict_pos {
                entries.remove(pos);
                self.total.fetch_sub(1, Ordering::Relaxed);
                FS_STATS.evictions.fetch_add(1, Ordering::Relaxed);
            }
        }

        entries.push(InodeCacheEntry { sb_id, ino, inode, access_tick: tick, referenced: true });
        self.total.fetch_add(1, Ordering::Relaxed);
    }

    /// Retire un inode du cache.
    pub fn remove(&self, sb_id: u32, ino: InodeNumber) {
        let idx = self.hash(sb_id, ino);
        let mut entries = self.buckets[idx].entries.lock();
        let before = entries.len();
        entries.retain(|e| !(e.sb_id == sb_id && e.ino == ino));
        if entries.len() < before {
            self.total.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Vide tous les inodes d'un superbloc (lors du démontage).
    pub fn evict_superblock(&self, sb_id: u32) {
        for bucket in self.buckets.iter() {
            let mut entries = bucket.entries.lock();
            let before = entries.len();
            entries.retain(|e| e.sb_id != sb_id);
            let removed = before - entries.len();
            self.total.fetch_sub(removed, Ordering::Relaxed);
        }
    }

    /// Nombre d'inodes en cache.
    pub fn total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }

    /// Identifie les inodes dirty (pour writeback en arrière-plan).
    pub fn collect_dirty(&self, sb_id: u32) -> Vec<InodeRef> {
        let mut dirty = Vec::new();
        for bucket in self.buckets.iter() {
            let entries = bucket.entries.lock();
            for e in entries.iter() {
                if e.sb_id != sb_id { continue; }
                let inode = e.inode.read();
                if inode.state == crate::fs::core::inode::InodeState::Dirty {
                    dirty.push(e.inode.clone());
                }
            }
        }
        dirty
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GlobalInodeCache — singleton thread-safe
// ─────────────────────────────────────────────────────────────────────────────

use core::cell::UnsafeCell;
use core::sync::atomic::AtomicBool;

pub struct GlobalInodeCache {
    inner: UnsafeCell<Option<InodeHashCache>>,
    ready: AtomicBool,
}

// SAFETY: accès contrôlé par `ready` + init unique au boot.
unsafe impl Send for GlobalInodeCache {}
unsafe impl Sync for GlobalInodeCache {}

impl GlobalInodeCache {
    pub const fn uninit() -> Self {
        Self { inner: UnsafeCell::new(None), ready: AtomicBool::new(false) }
    }

    pub fn init(&self, cap: usize) {
        if self.ready.swap(true, Ordering::SeqCst) {
            panic!("GlobalInodeCache::init appelé deux fois");
        }
        unsafe {
            use core::mem::MaybeUninit;
            let mut arr: [MaybeUninit<InodeBucket>; ICACHE_BUCKETS] =
                MaybeUninit::uninit().assume_init();
            for slot in arr.iter_mut() {
                slot.write(InodeBucket::new());
            }
            // SAFETY: tous les slots sont initialisés.
            let buckets = core::mem::transmute::<
                [MaybeUninit<InodeBucket>; ICACHE_BUCKETS],
                [InodeBucket; ICACHE_BUCKETS]
            >(arr);
            *self.inner.get() = Some(InodeHashCache {
                buckets,
                total:      AtomicUsize::new(0),
                cap,
                clock_tick: AtomicU64::new(0),
            });
        }
    }

    #[inline(always)]
    pub fn get(&self) -> &InodeHashCache {
        // SAFETY: stable après init().
        unsafe {
            (*self.inner.get()).as_ref()
                .expect("INODE_HASH_CACHE non initialisé — appeler inode_cache_init() au boot")
        }
    }
}

pub static INODE_HASH_CACHE: GlobalInodeCache = GlobalInodeCache::uninit();

pub fn inode_cache_init(cap: usize) {
    INODE_HASH_CACHE.init(cap);
}
