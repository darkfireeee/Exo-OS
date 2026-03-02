// kernel/src/memory/physical/allocator/slub.rs
//
// Allocateur SLUB — variante optimisée du slab avec fragmentation réduite.
// Utilise des slabs partiels sur des pages complètes, sans coloration de cache
// mais avec une gestion plus fine des slabs partiels via une liste unique.
// Couche 0 — aucune dépendance externe sauf `spin`.

use core::mem;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicUsize, AtomicU64, AtomicBool, Ordering};
use spin::Mutex;

use crate::memory::core::{AllocError, AllocFlags, PhysAddr, PAGE_SIZE, CACHE_LINE_SIZE};
use crate::memory::physical::allocator::slab::{
    SlabPageProvider, SLAB_PAGE_PROVIDER, size_class_for, N_SIZE_CLASSES, SIZE_CLASSES, SizeClassInfo,
};

// ─────────────────────────────────────────────────────────────────────────────
// HEADER DE SLUB (compact, 32 octets)
// ─────────────────────────────────────────────────────────────────────────────

/// Header SLUB — plus compact que SlabHeader (32 octets vs 64).
/// Stocké au début de chaque page de slub.
///
/// La freelist est encodée directement : chaque objet libre sur 8 octets
/// préfixe un pointeur XOR-encodé (XOR avec une clé par slub) pour
/// résister aux attaques de type use-after-free.
#[repr(C, align(32))]
struct SlubHeader {
    /// Tête de la freelist (encodée XOR).
    freelist:    *mut u8,
    /// Clé XOR pour encoder/décoder la freelist.
    freelist_key: usize,
    /// Objets actifs dans ce slub.
    inuse:       u16,
    /// Objets totaux.
    total:       u16,
    /// Index de classe de taille.
    class_idx:   u8,
    _pad:        [u8; 3],
    /// Lien suivant/précédent dans la liste partielle.
    next:        *mut SlubHeader,
    prev:        *mut SlubHeader,
}

const _: () = assert!(mem::size_of::<SlubHeader>() <= 64,
    "SlubHeader trop grand");

impl SlubHeader {
    /// Encode/décode un pointeur de freelist avec XOR.
    #[inline]
    fn encode_ptr(ptr: *mut u8, key: usize) -> *mut u8 {
        ((ptr as usize) ^ key) as *mut u8
    }

    /// Lire le prochain élément de la freelist (décode XOR).
    ///
    /// SAFETY: `obj` est un pointeur valide vers un objet libre de ce slub.
    #[inline]
    unsafe fn read_next(&self, obj: *mut u8) -> *mut u8 {
        let raw = core::ptr::read(obj as *const *mut u8);
        Self::encode_ptr(raw, self.freelist_key)
    }

    /// Écrire le lien suivant dans la freelist d'un objet (encode XOR).
    ///
    /// SAFETY: `obj` est un pointeur valide vers un objet libre de ce slub.
    #[inline]
    unsafe fn write_next(&self, obj: *mut u8, next: *mut u8) {
        let encoded = Self::encode_ptr(next, self.freelist_key);
        core::ptr::write(obj as *mut *mut u8, encoded);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CACHE SLUB
// ─────────────────────────────────────────────────────────────────────────────

pub struct SlubCacheStats {
    pub allocs:        AtomicU64,
    pub frees:         AtomicU64,
    pub slubs_created: AtomicU64,
    pub slubs_reaped:  AtomicU64,
    pub current_inuse: AtomicUsize,
    pub oom_count:     AtomicU64,
}

impl SlubCacheStats {
    pub const fn new() -> Self {
        SlubCacheStats {
            allocs:        AtomicU64::new(0),
            frees:         AtomicU64::new(0),
            slubs_created: AtomicU64::new(0),
            slubs_reaped:  AtomicU64::new(0),
            current_inuse: AtomicUsize::new(0),
            oom_count:     AtomicU64::new(0),
        }
    }
}

pub struct SlubCache {
    info:    SizeClassInfo,
    inner:   Mutex<SlubCacheInner>,
    pub stats:   SlubCacheStats,
    enabled: AtomicBool,
}

struct SlubCacheInner {
    /// Liste partielle : slubs avec des objets libres ET alloués.
    partial: *mut SlubHeader,
    partial_count: usize,
    /// Liste vide : slubs entièrement libres.
    empty:   *mut SlubHeader,
    empty_count:   usize,
    /// Nombre de slubs complets (sans pointeur — on ne les suit pas).
    full_count:    usize,
    // Note : plus de vmalloc_bump — adresse virtuelle dérivée de phys via physmap
}

// SAFETY: SlubCache est protégé par son Mutex interne.
unsafe impl Sync for SlubCache {}
unsafe impl Send for SlubCache {}

impl SlubCache {
    pub const fn new(info: SizeClassInfo) -> Self {
        SlubCache {
            info,
            inner: Mutex::new(SlubCacheInner {
                partial:       core::ptr::null_mut(),
                partial_count: 0,
                empty:         core::ptr::null_mut(),
                empty_count:   0,
                full_count:    0,
            }),
            stats:   SlubCacheStats::new(),
            enabled: AtomicBool::new(false),
        }
    }

    pub fn enable(&self) { self.enabled.store(true, Ordering::Release); }

    /// Alloue un objet de ce cache SLUB.
    pub fn alloc(&self, _flags: AllocFlags) -> Result<NonNull<u8>, AllocError> {
        if !self.enabled.load(Ordering::Acquire) {
            return Err(AllocError::NotInitialized);
        }
        let mut inner = self.inner.lock();

        // 1. Essayer la liste partielle
        if let Some(ptr) = self.alloc_partial(&mut inner) {
            self.stats.allocs.fetch_add(1, Ordering::Relaxed);
            self.stats.current_inuse.fetch_add(1, Ordering::Relaxed);
            return Ok(ptr);
        }

        // 2. Réactiver un slub vide
        if let Some(ptr) = self.reactivate_empty(&mut inner) {
            self.stats.allocs.fetch_add(1, Ordering::Relaxed);
            self.stats.current_inuse.fetch_add(1, Ordering::Relaxed);
            return Ok(ptr);
        }

        // 3. Nouveau slub
        match self.create_new_slub(&mut inner) {
            Ok(ptr) => {
                self.stats.slubs_created.fetch_add(1, Ordering::Relaxed);
                self.stats.allocs.fetch_add(1, Ordering::Relaxed);
                self.stats.current_inuse.fetch_add(1, Ordering::Relaxed);
                Ok(ptr)
            }
            Err(e) => {
                self.stats.oom_count.fetch_add(1, Ordering::Relaxed);
                Err(e)
            }
        }
    }

    /// Libère un objet de ce cache SLUB.
    ///
    /// SAFETY: `ptr` doit avoir été alloué via ce cache et ne plus être utilisé.
    pub unsafe fn free(&self, ptr: NonNull<u8>) {
        let mut inner = self.inner.lock();
        let page_start = ptr.as_ptr() as usize & !(PAGE_SIZE - 1);
        let header = page_start as *mut SlubHeader;

        // SAFETY: header pointe sur un SlubHeader géré par ce cache.
        (*header).write_next(ptr.as_ptr(), (*header).freelist);
        (*header).freelist = ptr.as_ptr();
        (*header).inuse   -= 1;

        let inuse = (*header).inuse;
        let total = (*header).total;

        let r = &mut *inner;
        if inuse == 0 {
            unsafe {
                slub_list_remove(header, &mut r.partial, &mut r.partial_count);
                slub_list_push_front(header, &mut r.empty, &mut r.empty_count);
            }
            self.stats.current_inuse.fetch_sub(1, Ordering::Relaxed);
            self.stats.frees.fetch_add(1, Ordering::Relaxed);
        } else if inuse + 1 == total {
            // Était plein (sans liste), maintenant partiel
            r.full_count = r.full_count.saturating_sub(1);
            unsafe { slub_list_push_front(header, &mut r.partial, &mut r.partial_count); }
            self.stats.current_inuse.fetch_sub(1, Ordering::Relaxed);
            self.stats.frees.fetch_add(1, Ordering::Relaxed);
        } else {
            self.stats.current_inuse.fetch_sub(1, Ordering::Relaxed);
            self.stats.frees.fetch_add(1, Ordering::Relaxed);
        }
    }

    // ─── helpers privés ─────────────────────────────────────────────────────

    fn alloc_partial(&self, inner: &mut SlubCacheInner) -> Option<NonNull<u8>> {
        if inner.partial.is_null() { return None; }
        // SAFETY: inner.partial pointe sur un SlubHeader valide.
        let header = unsafe { &mut *inner.partial };
        if header.freelist.is_null() { return None; }

        let obj = header.freelist;
        // SAFETY: header est valide, obj pointe sur un objet libre encodé XOR.
        let next = unsafe { header.read_next(obj) };
        header.freelist = next;
        header.inuse   += 1;

        if header.inuse == header.total {
            let h = inner.partial as *mut SlubHeader;
            unsafe { slub_list_remove(h, &mut inner.partial, &mut inner.partial_count); }
            inner.full_count += 1;
        }
        NonNull::new(obj)
    }

    fn reactivate_empty(&self, inner: &mut SlubCacheInner) -> Option<NonNull<u8>> {
        if inner.empty.is_null() { return None; }
        let h = inner.empty;
        unsafe {
            slub_list_remove(h, &mut inner.empty, &mut inner.empty_count);
            slub_list_push_front(h, &mut inner.partial, &mut inner.partial_count);
        }
        self.alloc_partial(inner)
    }

    fn create_new_slub(&self, inner: &mut SlubCacheInner) -> Result<NonNull<u8>, AllocError> {
        let phys = SLAB_PAGE_PROVIDER.get_page()?;
        // Adresse virtuelle = physmap directe (PHYS_MAP_BASE + phys).
        // La physmap couvre l'intégralité de la RAM physique sans mapping supplémentaire.
        let virt_base = crate::memory::core::layout::PHYS_MAP_BASE.as_u64() + phys.as_u64();
        let _ = phys; // phys est désormais encodé dans virt_base

        // Générer une clé XOR pseudo-aléatoire basée sur l'adresse virtuelle
        let key = (virt_base as usize).wrapping_mul(0x9e3779b97f4a7c15);

        let header_end  = mem::size_of::<SlubHeader>();
        let first_off   = align_up_slub(header_end, self.info.alignment);
        let usable      = PAGE_SIZE - first_off;
        let n_objs      = usable / self.info.size;

        // SAFETY: virt_base pointe sur une page valide fraîchement mappée.
        unsafe {
            // Construire la freelist XOR-encodée
            for i in 0..n_objs {
                let obj  = (virt_base as usize + first_off + i * self.info.size) as *mut u8;
                let next = if i + 1 < n_objs {
                    (virt_base as usize + first_off + (i + 1) * self.info.size) as *mut u8
                } else {
                    core::ptr::null_mut()
                };
                let encoded = ((next as usize) ^ key) as *mut u8;
                core::ptr::write(obj as *mut *mut u8, encoded);
            }

            let header = virt_base as *mut SlubHeader;
            let first_obj = (virt_base as usize + first_off) as *mut u8;
            core::ptr::write(header, SlubHeader {
                freelist:     first_obj,
                freelist_key: key,
                inuse:        0,
                total:        n_objs as u16,
                class_idx:    0,
                _pad:         [0; 3],
                next:         core::ptr::null_mut(),
                prev:         core::ptr::null_mut(),
            });
            slub_list_push_front(header, &mut inner.partial, &mut inner.partial_count);
        }

        self.alloc_partial(inner).ok_or(AllocError::OutOfMemory)
    }
}

#[inline]
fn align_up_slub(val: usize, align: usize) -> usize {
    (val + align - 1) & !(align - 1)
}

// ─────────────────────────────────────────────────────────────────────────────
// OPÉRATIONS SUR LES LISTES
// ─────────────────────────────────────────────────────────────────────────────

unsafe fn slub_list_push_front(node: *mut SlubHeader, head: &mut *mut SlubHeader, count: &mut usize) {
    (*node).prev = core::ptr::null_mut();
    (*node).next = *head;
    if !(*head).is_null() { (*(*head)).prev = node; }
    *head  = node;
    *count += 1;
}

unsafe fn slub_list_remove(node: *mut SlubHeader, head: &mut *mut SlubHeader, count: &mut usize) {
    if node.is_null() { return; }
    let prev = (*node).prev;
    let next = (*node).next;
    if !prev.is_null() { (*prev).next = next; } else { *head = next; }
    if !next.is_null() { (*next).prev = prev; }
    (*node).prev  = core::ptr::null_mut();
    (*node).next  = core::ptr::null_mut();
    *count = count.saturating_sub(1);
}

// ─────────────────────────────────────────────────────────────────────────────
// TABLE GLOBALE DE CACHES SLUB
// ─────────────────────────────────────────────────────────────────────────────

macro_rules! slub_caches_init {
    ($($size:expr, $align:expr);*) => {
        [$(SlubCache::new(SizeClassInfo::new($size, $align)),)*]
    }
}

pub static SLUB_CACHES: [SlubCache; N_SIZE_CLASSES] = slub_caches_init! {
       8,  8;
      16, 16;
      32, 16;
      64, 64;
     128, 64;
     256, 64;
     512, 64;
    1024, 64;
    2048, 64
};

pub fn init_all() {
    for cache in &SLUB_CACHES { cache.enable(); }
}

pub fn alloc(size: usize, flags: AllocFlags) -> Result<NonNull<u8>, AllocError> {
    let idx = size_class_for(size).ok_or(AllocError::InvalidParams)?;
    SLUB_CACHES[idx].alloc(flags)
}

/// SAFETY: `ptr` doit avoir été alloué via `alloc()` avec la même `size`.
pub unsafe fn free(ptr: NonNull<u8>, size: usize) {
    if let Some(idx) = size_class_for(size) {
        SLUB_CACHES[idx].free(ptr);
    }
}
