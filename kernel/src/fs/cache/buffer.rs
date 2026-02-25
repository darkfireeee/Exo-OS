// kernel/src/fs/cache/buffer.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// BUFFER CACHE — Cache bloc (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Le buffer cache stocke les blocs de métadonnées et les blocs de données
// bruts lus depuis un block device (avant d'être traduits en pages VFS).
// Principalement utilisé par ext4plus pour les group descriptors, le journal,
// le bitmap d'inodes et le bitmap de blocs.
//
// Structure :
//   • BufHead = buffer header (bloc + état + données)
//   • BufferCache = hash table shardée indexée par (dev, block_number)
//   • Les BufHead dirty sont ajoutés à une dirty_list pour le writeback
//   • Uptodate + Lock bits pour synchronisation I/O
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fs::core::types::{DevId, FsError, FsResult, FS_STATS};
use crate::memory::core::types::PhysAddr;
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Taille d'un buffer standard = 4 KiB (peut être différent de PAGE_SIZE pour
/// des FS avec des blocs plus petits, non supporté ici — simplifié à 4 KiB).
pub const BUF_SIZE: usize = 4096;
/// Buckets dans la hash table.
const BUF_BUCKETS: usize = 1024;
const BUF_MASK: usize    = BUF_BUCKETS - 1;

// ─────────────────────────────────────────────────────────────────────────────
// BlockNumber — numéro de bloc physique
// ─────────────────────────────────────────────────────────────────────────────

/// Numéro de bloc physique sur un block device.
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct BlockNumber(pub u64);

impl BlockNumber {
    #[inline(always)]
    pub const fn new(n: u64) -> Self { BlockNumber(n) }
    #[inline(always)]
    pub const fn as_u64(self) -> u64 { self.0 }

    /// Byte offset sur le device.
    #[inline(always)]
    pub const fn byte_offset(self, blksize: u32) -> u64 {
        self.0 * blksize as u64
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BufHead — buffer header
// ─────────────────────────────────────────────────────────────────────────────

/// Buffer header : état d'un bloc de données mis en cache.
pub struct BufHead {
    /// Device auquel appartient ce bloc.
    pub dev:     DevId,
    /// Numéro de bloc.
    pub block:   BlockNumber,
    /// Taille du bloc en octets.
    pub blksize: u32,
    /// Adresse physique de la frame allouée.
    pub phys:    PhysAddr,
    /// Pointeur vers les données (kernel mapping).
    pub data:    *mut u8,
    /// Données à jour depuis le disque.
    pub uptodate: AtomicBool,
    /// Bloc modifié, à écrire (dirty).
    pub dirty:   AtomicBool,
    /// Verrou d'I/O (locked pendant un transfert).
    pub io_lock: AtomicBool,
    /// Erreur I/O.
    pub error:   AtomicBool,
    /// Compteur de références.
    pub refcount: AtomicU32,
    /// Tick d'accès pour LRU.
    pub access_tick: AtomicU64,
}

// SAFETY: data est une frame kernel — accès contrôlé via lock externe.
unsafe impl Send for BufHead {}
unsafe impl Sync for BufHead {}

impl BufHead {
    /// Crée un BufHead vierge (non uptodate).
    pub fn new(dev: DevId, block: BlockNumber, blksize: u32, phys: PhysAddr, data: *mut u8) -> Self {
        Self {
            dev,
            block,
            blksize,
            phys,
            data,
            uptodate:    AtomicBool::new(false),
            dirty:       AtomicBool::new(false),
            io_lock:     AtomicBool::new(false),
            error:       AtomicBool::new(false),
            refcount:    AtomicU32::new(1),
            access_tick: AtomicU64::new(0),
        }
    }

    /// Lecture des données (tranche immuable).
    ///
    /// # Safety
    /// Le buffer doit être `uptodate` et aucun I/O en cours.
    pub unsafe fn as_slice(&self) -> &[u8] {
        // SAFETY: data est une frame kernel valide de taille `blksize`.
        core::slice::from_raw_parts(self.data, self.blksize as usize)
    }

    /// Écriture dans les données (tranche mutable).
    ///
    /// # Safety
    /// Le buffer doit être verrouillé par l'appelant.
    pub unsafe fn as_mut_slice(&self) -> &mut [u8] {
        // SAFETY: data est une frame kernel valide, accès exclusif garanti par caller.
        core::slice::from_raw_parts_mut(self.data, self.blksize as usize)
    }

    /// Marque comme dirty.
    #[inline(always)]
    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Release);
    }

    /// Marque comme propre après l'écriture.
    #[inline(always)]
    pub fn mark_clean(&self) {
        self.dirty.store(false, Ordering::Release);
    }

    /// Essai d'acquisition du lock I/O (non bloquant).
    pub fn try_lock_io(&self) -> bool {
        self.io_lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok()
    }

    /// Libère le lock I/O.
    pub fn unlock_io(&self) {
        self.io_lock.store(false, Ordering::Release);
    }

    /// Incrémente les références.
    pub fn get(&self) {
        self.refcount.fetch_add(1, Ordering::Relaxed);
    }

    /// Décrémente les références. Retourne vrai si dernier.
    pub fn put(&self) -> bool {
        self.refcount.fetch_sub(1, Ordering::AcqRel) == 1
    }

    /// Est évictable (refcount == 1, pas de I/O).
    pub fn is_evictable(&self) -> bool {
        self.refcount.load(Ordering::Relaxed) == 1
            && !self.io_lock.load(Ordering::Relaxed)
            && !self.dirty.load(Ordering::Relaxed)
    }
}

/// Référence partagée à un BufHead.
pub type BufRef = Arc<BufHead>;

// ─────────────────────────────────────────────────────────────────────────────
// BufferCache — hash table de BufHead
// ─────────────────────────────────────────────────────────────────────────────

struct BufBucketEntry {
    dev:   DevId,
    block: BlockNumber,
    buf:   BufRef,
}

struct BufBucket {
    entries: SpinLock<Vec<BufBucketEntry>>,
}

impl BufBucket {
    const fn new() -> Self { Self { entries: SpinLock::new(Vec::new()) } }
}

pub struct BufferCache {
    buckets:    [BufBucket; BUF_BUCKETS],
    total:      AtomicUsize,
    cap:        usize,
    clock_tick: AtomicU64,
}

impl BufferCache {
    #[inline(always)]
    fn hash(&self, dev: DevId, block: BlockNumber) -> usize {
        let h = dev.0.wrapping_mul(0x9e3779b97f4a7c15)
            ^ block.0.wrapping_mul(0x6c62272e07bb0142);
        (h as usize) & BUF_MASK
    }

    /// Recherche un buffer.
    pub fn lookup(&self, dev: DevId, block: BlockNumber) -> Option<BufRef> {
        let idx = self.hash(dev, block);
        let tick = self.clock_tick.fetch_add(1, Ordering::Relaxed);
        let entries = self.buckets[idx].entries.lock();
        for e in entries.iter() {
            if e.dev == dev && e.block == block {
                e.buf.access_tick.store(tick, Ordering::Relaxed);
                e.buf.get();
                FS_STATS.cache_hits.fetch_add(1, Ordering::Relaxed);
                return Some(e.buf.clone());
            }
        }
        FS_STATS.cache_misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Insère un buffer.
    pub fn insert(&self, buf: BufRef) {
        let dev   = buf.dev;
        let block = buf.block;
        let idx   = self.hash(dev, block);
        let tick  = self.clock_tick.fetch_add(1, Ordering::Relaxed);
        let mut entries = self.buckets[idx].entries.lock();

        entries.retain(|e| !(e.dev == dev && e.block == block));

        if self.cap > 0 && self.total.load(Ordering::Relaxed) >= self.cap {
            if let Some(pos) = entries.iter().position(|e| e.buf.is_evictable()) {
                entries.remove(pos);
                self.total.fetch_sub(1, Ordering::Relaxed);
                FS_STATS.evictions.fetch_add(1, Ordering::Relaxed);
            }
        }

        buf.access_tick.store(tick, Ordering::Relaxed);
        entries.push(BufBucketEntry { dev, block, buf });
        self.total.fetch_add(1, Ordering::Relaxed);
    }

    /// Retire un buffer du cache.
    pub fn remove(&self, dev: DevId, block: BlockNumber) {
        let idx = self.hash(dev, block);
        let mut entries = self.buckets[idx].entries.lock();
        let before = entries.len();
        entries.retain(|e| !(e.dev == dev && e.block == block));
        if entries.len() < before {
            self.total.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Invalide tous les buffers d'un device (umount).
    pub fn invalidate_device(&self, dev: DevId) {
        for bucket in self.buckets.iter() {
            let mut entries = bucket.entries.lock();
            let before = entries.len();
            entries.retain(|e| e.dev != dev);
            let removed = before - entries.len();
            self.total.fetch_sub(removed, Ordering::Relaxed);
        }
    }

    /// Collecte les buffers dirty pour writeback.
    pub fn collect_dirty(&self, dev: DevId) -> Vec<BufRef> {
        let mut dirty = Vec::new();
        for bucket in self.buckets.iter() {
            let entries = bucket.entries.lock();
            for e in entries.iter() {
                if e.dev == dev && e.buf.dirty.load(Ordering::Relaxed) {
                    dirty.push(e.buf.clone());
                }
            }
        }
        dirty
    }

    pub fn total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Singleton global
// ─────────────────────────────────────────────────────────────────────────────

use core::cell::UnsafeCell;

pub struct GlobalBufferCache {
    inner: UnsafeCell<Option<BufferCache>>,
    ready: AtomicBool,
}

unsafe impl Send for GlobalBufferCache {}
unsafe impl Sync for GlobalBufferCache {}

impl GlobalBufferCache {
    pub const fn uninit() -> Self {
        Self { inner: UnsafeCell::new(None), ready: AtomicBool::new(false) }
    }

    pub fn init(&self, cap: usize) {
        if self.ready.swap(true, Ordering::SeqCst) {
            panic!("GlobalBufferCache::init appelé deux fois");
        }
        unsafe {
            use core::mem::MaybeUninit;
            let mut arr: [MaybeUninit<BufBucket>; BUF_BUCKETS] =
                MaybeUninit::uninit().assume_init();
            for slot in arr.iter_mut() { slot.write(BufBucket::new()); }
            let buckets = core::mem::transmute::<
                [MaybeUninit<BufBucket>; BUF_BUCKETS],
                [BufBucket; BUF_BUCKETS]
            >(arr);
            *self.inner.get() = Some(BufferCache {
                buckets,
                total:      AtomicUsize::new(0),
                cap,
                clock_tick: AtomicU64::new(0),
            });
        }
    }

    #[inline(always)]
    pub fn get(&self) -> &BufferCache {
        // SAFETY: stable après init().
        unsafe {
            (*self.inner.get()).as_ref()
                .expect("BUFFER_CACHE non initialisé — appeler buffer_cache_init() au boot")
        }
    }
}

pub static BUFFER_CACHE: GlobalBufferCache = GlobalBufferCache::uninit();

pub fn buffer_cache_init(cap: usize) {
    BUFFER_CACHE.init(cap);
}
