// kernel/src/fs/cache/page_cache.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// PAGE CACHE — Cache de pages LRU + prefetch (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Le page cache stocke les pages de données des fichiers en mémoire.
// Chaque entrée = (InodeNumber + page_index) → frame physique.
//
// Architecture :
//   • Hash table shardée (BUCKETS buckets, spinlock par bucket)
//   • LRU clock-hand pour l'éviction (O(1) amortized)
//   • Compteur de références : 0 = évictable, >0 = épinglé (DMA livraison)
//   • Dirty tracking : bit dirty par page + writequeue
//   • Prefetch adaptatif : voir prefetch.rs
//
// Instrumentation complète :
//   FS_STATS.page_cache_count, cache_hits, cache_misses, evictions, writes_bytes…
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fs::core::types::{
    FsError, FsResult, InodeNumber, DevId, FS_STATS, FS_BLOCK_SIZE,
};
use crate::memory::core::types::PhysAddr;
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Taille d'une page de cache = taille de page système (4 KiB).
pub const PAGE_CACHE_PAGE_SIZE: u64 = 4096;
/// Décalage logarithmique.
pub const PAGE_CACHE_SHIFT: u32 = 12;
/// Nombre de buckets dans la hash table.
const PCACHE_BUCKETS: usize = 2048;
const PCACHE_MASK: usize    = PCACHE_BUCKETS - 1;
/// TTL min d'une page propre avant éviction forcée (en unités de clock tick).
const PAGE_MIN_RESIDENT_TICKS: u64 = 10;

// ─────────────────────────────────────────────────────────────────────────────
// PageIndex — index d'une page (offset_in_file / PAGE_CACHE_PAGE_SIZE)
// ─────────────────────────────────────────────────────────────────────────────

/// Index de page dans un fichier (numéro de page à partir de 0).
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct PageIndex(pub u64);

impl PageIndex {
    /// Calcule l'offset en octets dans le fichier.
    #[inline(always)]
    pub const fn byte_offset(self) -> u64 {
        self.0 << PAGE_CACHE_SHIFT
    }

    /// Construit depuis un offset en octets.
    #[inline(always)]
    pub const fn from_offset(off: u64) -> Self {
        PageIndex(off >> PAGE_CACHE_SHIFT)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PageState — état d'une page de cache
// ─────────────────────────────────────────────────────────────────────────────

/// État d'une page dans le page cache.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum PageState {
    /// Page propre, cohérente avec le disque.
    Clean,
    /// Page modifiée, à écrire sur disque.
    Dirty,
    /// Page en cours de lecture depuis disque (IO en cours).
    ReadPending,
    /// Page en cours d'écriture vers disque.
    WritePending,
    /// Page invalide (après truncate ou erreur).
    Invalid,
}

// ─────────────────────────────────────────────────────────────────────────────
// PageFlags — flags binaires d'état d'une page (DIRTY, WRITEBACK…)
// ─────────────────────────────────────────────────────────────────────────────

/// Bits de flags d'état d'une [`CachedPage`] stockés dans `flags: AtomicU32`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PageFlags(pub u32);

impl PageFlags {
    /// La page a été modifiée et doit être écrite sur disque.
    pub const DIRTY:     PageFlags = PageFlags(1 << 0);
    /// La page est en cours d'écriture (writeback actif).
    pub const WRITEBACK: PageFlags = PageFlags(1 << 1);
    /// La page est verrouillée (I/O en cours).
    pub const LOCKED:    PageFlags = PageFlags(1 << 2);
    /// La page est en cours d'allocation.
    pub const UPTODATE:  PageFlags = PageFlags(1 << 3);

    /// Retourne la valeur brute u32.
    #[inline(always)]
    pub const fn bits(self) -> u32 { self.0 }
}

// ─────────────────────────────────────────────────────────────────────────────
// CachedPage — une page dans le cache
// ─────────────────────────────────────────────────────────────────────────────

/// Une page de données en cache.
pub struct CachedPage {
    /// Numéro d'inode propriétaire.
    pub ino:        InodeNumber,
    /// Index de page dans le fichier.
    pub idx:        PageIndex,
    /// Adresse physique de la frame (allouée par buddy allocator).
    pub phys:       PhysAddr,
    /// Adresse virtuelle (mapping kernel).
    pub virt:       *mut u8,
    /// Nombre de références (épinglage DMA/mmap).
    pub refcount:   AtomicU32,
    /// Dirty flag.
    pub dirty:      AtomicBool,
    /// Flags binaires (DIRTY, WRITEBACK, LOCKED, UPTODATE).
    pub flags:      AtomicU32,
    /// État de la page.
    pub state:      PageState,
    /// Tick d'insertion (pour LRU).
    pub insert_tick: AtomicU64,
    /// Tick d'accès (pour LRU).
    pub access_tick: AtomicU64,
    /// Bit de référence pour clock algorithm.
    pub referenced: AtomicBool,
    /// Erreur I/O capturée.
    pub io_error:   AtomicBool,
    /// La page contient des données valides (à jour depuis le disque).
    pub uptodate:   AtomicBool,
    /// Device propriétaire de la page.
    pub dev:        DevId,
}

// SAFETY: virt est une adresse kernel mappée — le pointeur brut n'est utilisé
// que depuis du code kernel synchronisé via le lock du bucket.
unsafe impl Send for CachedPage {}
unsafe impl Sync for CachedPage {}

impl CachedPage {
    /// Crée une page avec une adresse physique et virtuelle données.
    pub fn new(ino: InodeNumber, idx: PageIndex, phys: PhysAddr, virt: *mut u8, now_tick: u64) -> Self {
        FS_STATS.page_cache_count.fetch_add(1, Ordering::Relaxed);
        Self {
            ino,
            idx,
            phys,
            virt,
            refcount:    AtomicU32::new(0),
            dirty:       AtomicBool::new(false),
            flags:       AtomicU32::new(0),
            state:       PageState::Clean,
            insert_tick: AtomicU64::new(now_tick),
            access_tick: AtomicU64::new(now_tick),
            referenced:  AtomicBool::new(true),
            io_error:    AtomicBool::new(false),
            uptodate:    AtomicBool::new(false),
            dev:         DevId::NONE,
        }
    }

    /// Épingle la page (incrémente refcount).
    #[inline(always)]
    pub fn pin(&self) -> u32 {
        self.refcount.fetch_add(1, Ordering::Acquire) + 1
    }

    /// Relâche la page (décrémente refcount). Retourne vrai si dernier épingle.
    #[inline(always)]
    pub fn unpin(&self) -> bool {
        self.refcount.fetch_sub(1, Ordering::Release) == 1
    }

    /// Retourne vrai si la page est évictable (refcount == 0, pas de IO en cours).
    #[inline(always)]
    pub fn is_evictable(&self) -> bool {
        self.refcount.load(Ordering::Relaxed) == 0
            && self.state != PageState::WritePending
            && self.state != PageState::ReadPending
    }

    /// Marque la page dirty (après écriture).
    #[inline(always)]
    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Release);
    }

    /// Marque la page propre (après write-back).
    #[inline(always)]
    pub fn mark_clean(&self) {
        self.dirty.store(false, Ordering::Release);
    }

    /// Retourne l'adresse physique de la frame.
    #[inline(always)]
    pub fn phys_addr(&self) -> PhysAddr {
        self.phys
    }

    /// Met à jour le tick d'accès.
    #[inline(always)]
    pub fn touch(&self, now_tick: u64) {
        self.access_tick.store(now_tick, Ordering::Relaxed);
        self.referenced.store(true, Ordering::Relaxed);
    }

    /// Copie `len` octets depuis `buf` dans la page à l'offset `off_in_page`.
    ///
    /// # Safety
    /// `off_in_page + len ≤ PAGE_CACHE_PAGE_SIZE` doit être garanti par l'appelant.
    pub unsafe fn write_data(&self, off_in_page: usize, buf: &[u8]) {
        debug_assert!(off_in_page + buf.len() <= PAGE_CACHE_PAGE_SIZE as usize);
        // SAFETY: virt est une frame kernel valide → la plage est écrivable.
        let dst = self.virt.add(off_in_page);
        core::ptr::copy_nonoverlapping(buf.as_ptr(), dst, buf.len());
        self.mark_dirty();
    }

    /// Copie `len` octets depuis la page vers `buf` à l'offset `off_in_page`.
    ///
    /// # Safety
    /// `off_in_page + len ≤ PAGE_CACHE_PAGE_SIZE` doit être garanti par l'appelant.
    pub unsafe fn read_data(&self, off_in_page: usize, buf: &mut [u8]) {
        debug_assert!(off_in_page + buf.len() <= PAGE_CACHE_PAGE_SIZE as usize);
        // SAFETY: virt est une frame kernel valide → la plage est lisible.
        let src = self.virt.add(off_in_page);
        core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), buf.len());
    }
}

impl Drop for CachedPage {
    fn drop(&mut self) {
        FS_STATS.page_cache_count.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Référence partagée vers une `CachedPage`.
pub type PageRef = Arc<CachedPage>;

// ─────────────────────────────────────────────────────────────────────────────
// BucketEntry
// ─────────────────────────────────────────────────────────────────────────────

pub struct BucketEntry {
    pub ino: InodeNumber,
    pub idx: PageIndex,
    pub page: PageRef,
}

// ─────────────────────────────────────────────────────────────────────────────
// PageCache — hash table LRU shardée
// ─────────────────────────────────────────────────────────────────────────────

/// Page cache global VFS.
pub struct PageCache {
    pub buckets:     [PageCacheBucket; PCACHE_BUCKETS],
    /// Nombre total de pages en cache.
    pub total:       AtomicUsize,
    /// Limite maximum de pages (0 = illimité).
    cap:         usize,
    /// Tick monotone (incrémenté à chaque accès batch).
    clock_tick:  AtomicU64,
}

pub struct PageCacheBucket {
    pub entries: SpinLock<Vec<BucketEntry>>,
}

impl PageCacheBucket {
    const fn new() -> Self {
        Self { entries: SpinLock::new(Vec::new()) }
    }

    /// Verrouille le bucket et retourne le guard.
    #[inline(always)]
    pub fn lock(&self) -> crate::scheduler::sync::spinlock::SpinLockGuard<'_, Vec<BucketEntry>> {
        self.entries.lock()
    }
}

impl PageCache {
    #[inline(always)]
    fn hash(&self, ino: InodeNumber, idx: PageIndex) -> usize {
        let h = ino.as_u64().wrapping_mul(0x9e3779b97f4a7c15)
            ^ idx.0.wrapping_mul(0x6c62272e07bb0142);
        (h as usize) & PCACHE_MASK
    }

    /// Recherche une page dans le cache. O(n bucket) — bucket ~3-4 entries en moyenne.
    pub fn lookup(&self, ino: InodeNumber, idx: PageIndex) -> Option<PageRef> {
        let bucket_idx = self.hash(ino, idx);
        let entries = self.buckets[bucket_idx].entries.lock();
        for e in entries.iter().rev() {
            if e.ino == ino && e.idx == idx {
                let tick = self.clock_tick.fetch_add(0, Ordering::Relaxed);
                e.page.touch(tick);
                FS_STATS.cache_hits.fetch_add(1, Ordering::Relaxed);
                return Some(e.page.clone());
            }
        }
        FS_STATS.cache_misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Insère une page dans le cache. Si la capacité est atteinte, évince une page propre.
    pub fn insert(&self, page: PageRef) -> FsResult<()> {
        let ino = page.ino;
        let idx = page.idx;
        let bucket_idx = self.hash(ino, idx);
        let mut entries = self.buckets[bucket_idx].entries.lock();

        // Supprimer l'éventuelle entrée existante.
        entries.retain(|e| !(e.ino == ino && e.idx == idx));

        // Gestion de la capacité.
        if self.cap > 0 && self.total.load(Ordering::Relaxed) >= self.cap {
            // Évincer la première page évictable non-dirty dans ce bucket.
            if let Some(pos) = entries.iter().position(|e| {
                e.page.is_evictable() && !e.page.dirty.load(Ordering::Relaxed)
            }) {
                entries.remove(pos);
                self.total.fetch_sub(1, Ordering::Relaxed);
                FS_STATS.evictions.fetch_add(1, Ordering::Relaxed);
            }
        }

        let tick = self.clock_tick.fetch_add(1, Ordering::Relaxed);
        page.insert_tick.store(tick, Ordering::Relaxed);
        entries.push(BucketEntry { ino, idx, page });
        self.total.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Retire une page du cache.
    pub fn remove(&self, ino: InodeNumber, idx: PageIndex) -> bool {
        let bucket_idx = self.hash(ino, idx);
        let mut entries = self.buckets[bucket_idx].entries.lock();
        let before = entries.len();
        entries.retain(|e| !(e.ino == ino && e.idx == idx));
        let removed = before != entries.len();
        if removed {
            self.total.fetch_sub(1, Ordering::Relaxed);
        }
        removed
    }

    /// Invalide toutes les pages d'un inode (truncate, unlink).
    pub fn invalidate_inode(&self, ino: InodeNumber) -> usize {
        let mut count = 0usize;
        for bucket in self.buckets.iter() {
            let mut entries = bucket.entries.lock();
            let before = entries.len();
            entries.retain(|e| e.ino != ino);
            let removed = before - entries.len();
            count += removed;
            self.total.fetch_sub(removed, Ordering::Relaxed);
        }
        count
    }

    /// Retourne toutes les pages dirty d'un inode pour le writeback.
    pub fn collect_dirty(&self, ino: InodeNumber) -> Vec<PageRef> {
        let mut dirty = Vec::new();
        for bucket in self.buckets.iter() {
            let entries = bucket.entries.lock();
            for e in entries.iter() {
                if e.ino == ino && e.page.dirty.load(Ordering::Relaxed) {
                    dirty.push(e.page.clone());
                }
            }
        }
        dirty
    }

    /// Retourne le nombre total de pages en cache.
    pub fn total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }

    /// Lit les données d'un fichier depuis le cache → `buf`.
    /// `offset` est l'offset en octets dans le fichier.
    /// Retourne le nombre d'octets copiés (peut être < buf.len() si fin de cache).
    pub fn read_pages(
        &self,
        ino:    InodeNumber,
        offset: u64,
        buf:    &mut [u8],
    ) -> usize {
        let mut copied = 0usize;
        let mut file_off = offset;

        while copied < buf.len() {
            let page_idx = PageIndex::from_offset(file_off);
            let off_in_page = (file_off & (PAGE_CACHE_PAGE_SIZE - 1)) as usize;
            let to_copy = (buf.len() - copied)
                .min(PAGE_CACHE_PAGE_SIZE as usize - off_in_page);

            match self.lookup(ino, page_idx) {
                Some(page) => {
                    // SAFETY: off_in_page + to_copy ≤ PAGE_CACHE_PAGE_SIZE garanti par .min().
                    unsafe { page.read_data(off_in_page, &mut buf[copied..copied + to_copy]); }
                    copied   += to_copy;
                    file_off += to_copy as u64;
                    FS_STATS.reads_bytes.fetch_add(to_copy as u64, Ordering::Relaxed);
                }
                None => break, // page manquante → le caller doit lire depuis disque
            }
        }
        copied
    }

    /// Écrit les données depuis `buf` vers les pages en cache.
    /// Crée les pages manquantes via `page_factory`.
    pub fn write_pages(
        &self,
        ino:           InodeNumber,
        offset:        u64,
        buf:           &[u8],
        page_factory:  &mut dyn FnMut(InodeNumber, PageIndex) -> FsResult<PageRef>,
    ) -> FsResult<usize> {
        let mut written = 0usize;
        let mut file_off = offset;
        let tick = self.clock_tick.fetch_add(1, Ordering::Relaxed);

        while written < buf.len() {
            let page_idx = PageIndex::from_offset(file_off);
            let off_in_page = (file_off & (PAGE_CACHE_PAGE_SIZE - 1)) as usize;
            let to_copy = (buf.len() - written)
                .min(PAGE_CACHE_PAGE_SIZE as usize - off_in_page);

            let page = match self.lookup(ino, page_idx) {
                Some(p) => p,
                None => {
                    let p = page_factory(ino, page_idx)?;
                    self.insert(p.clone())?;
                    p
                }
            };

            // SAFETY: off_in_page + to_copy ≤ PAGE_CACHE_PAGE_SIZE garanti par .min().
            unsafe { page.write_data(off_in_page, &buf[written..written + to_copy]); }
            page.touch(tick);
            written  += to_copy;
            file_off += to_copy as u64;
            FS_STATS.writes_bytes.fetch_add(to_copy as u64, Ordering::Relaxed);
        }
        Ok(written)
    }
}

// Initialisation statique — buckets
macro_rules! empty_pcache_buckets {
    () => { {
        // Trick pour initialiser [PageCacheBucket; 2048] en const context.
        // Rust ne permet pas `[expr; N]` pour N > 32 en const si le type
        // n'est pas Copy. On utilise un unsafe transmute depuis un tableau
        // de MaybeUninit initialisés manuellement.
        //
        // Pour this no_std kernel: on utilise un lazy_static pattern via
        // une fonction d'initialisation appelée au boot.
        //
        // En attendant la stabilisation de const-generic arrays, on limite
        // à une structure initialisée dynamiquement.
        unreachable!()
    } }
}

// ─────────────────────────────────────────────────────────────────────────────
// GlobalPageCache — wrapper singleton
// ─────────────────────────────────────────────────────────────────────────────

use core::cell::UnsafeCell;

/// Wrapper pour l'initialisation différée du page cache global.
pub struct GlobalPageCache {
    inner: UnsafeCell<Option<PageCache>>,
    ready: AtomicBool,
}

// SAFETY: l'accès est protégé par `ready` + initialisation une seule fois au boot.
unsafe impl Send for GlobalPageCache {}
unsafe impl Sync for GlobalPageCache {}

impl GlobalPageCache {
    pub const fn uninit() -> Self {
        Self { inner: UnsafeCell::new(None), ready: AtomicBool::new(false) }
    }

    /// Initialise le page cache. Appelé UNE SEULE fois au boot.
    pub fn init(&self, cap_pages: usize) {
        if self.ready.swap(true, Ordering::SeqCst) {
            panic!("GlobalPageCache::init appelé deux fois");
        }
        // SAFETY: init() est appelé une seule fois, pas d'accès concurrent.
        unsafe {
            // Construit dynamiquement le tableau de buckets.
            let buckets = {
                use core::mem::MaybeUninit;
                let mut arr: [MaybeUninit<PageCacheBucket>; PCACHE_BUCKETS] =
                    unsafe { MaybeUninit::uninit().assume_init() };
                for slot in arr.iter_mut() {
                    slot.write(PageCacheBucket::new());
                }
                // SAFETY: tous les slots sont initialisés.
                unsafe { core::mem::transmute::<_, [PageCacheBucket; PCACHE_BUCKETS]>(arr) }
            };
            *self.inner.get() = Some(PageCache {
                buckets,
                total:      AtomicUsize::new(0),
                cap:        cap_pages,
                clock_tick: AtomicU64::new(0),
            });
        }
    }

    /// Accès au page cache (panic si non initialisé).
    #[inline(always)]
    pub fn get(&self) -> &PageCache {
        // SAFETY: après init(), la référence est stable pour toute la durée du kernel.
        unsafe {
            (*self.inner.get()).as_ref()
                .expect("PAGE_CACHE non initialisé — appeler page_cache_init() au boot")
        }
    }

    /// Nombre total de pages dans le cache.
    #[inline(always)]
    pub fn total_pages(&self) -> usize {
        self.get().total.load(Ordering::Relaxed)
    }

    /// Nombre de pages dirty (pas encore écrites sur disque).
    #[inline(always)]
    pub fn dirty_pages(&self) -> usize {
        let pc = self.get();
        let mut count = 0usize;
        for bucket in pc.buckets.iter() {
            let entries = bucket.entries.lock();
            for e in entries.iter() {
                if e.page.dirty.load(Ordering::Relaxed) {
                    count += 1;
                }
            }
        }
        count
    }

    /// Itère sur tous les buckets du cache.
    #[inline(always)]
    pub fn iter_buckets(&self) -> core::slice::Iter<'_, PageCacheBucket> {
        self.get().buckets.iter()
    }

    /// Recherche une page dans le cache.
    #[inline(always)]
    pub fn lookup(&self, ino: InodeNumber, idx: PageIndex) -> Option<PageRef> {
        self.get().lookup(ino, idx)
    }

    /// Marque une page (ino, idx) comme dirty.
    pub fn mark_dirty(&self, ino: InodeNumber, idx: PageIndex) {
        if let Some(page) = self.get().lookup(ino, idx) {
            page.mark_dirty();
        }
    }
}

/// Page cache global.
pub static PAGE_CACHE: GlobalPageCache = GlobalPageCache::uninit();

/// Initialise le page cache global. Appelé par `fs::init()`.
pub fn page_cache_init(max_pages: usize) {
    PAGE_CACHE.init(max_pages);
}
