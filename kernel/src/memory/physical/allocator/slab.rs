// kernel/src/memory/physical/allocator/slab.rs
//
// Allocateur Slab — allocations de petits objets à taille fixe.
// Inspiré du slab allocator de Bonwick (1994) mais adapté no_std.
// Couche 0 — aucune dépendance externe sauf `spin`.

use core::mem;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicUsize, AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

use crate::memory::core::{
    AllocError, AllocFlags, PhysAddr, PAGE_SIZE, CACHE_LINE_SIZE,
};
use crate::memory::core::layout::PHYS_MAP_BASE;

// ─────────────────────────────────────────────────────────────────────────────
// CLASSES DE TAILLE SLAB
// ─────────────────────────────────────────────────────────────────────────────

/// Classes de taille supportées par le slab allocator.
/// Chaque classe gère des objets de taille fixe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeClassInfo {
    pub size:      usize,   // Taille de l'objet en octets
    pub alignment: usize,   // Alignement requis
    pub objs_per_slab: usize, // Nombre d'objets par slab page
    pub color_range:   usize, // Nombre de couleurs de cache
}

impl SizeClassInfo {
    pub const fn new(size: usize, alignment: usize) -> Self {
        let usable = PAGE_SIZE - mem::size_of::<SlabHeader>();
        let objs   = usable / size;
        let waste  = usable - objs * size;
        SizeClassInfo { size, alignment, objs_per_slab: objs, color_range: waste }
    }
}

/// Classes de taille standard (8 B → 2 KiB).
pub const SIZE_CLASSES: &[SizeClassInfo] = &[
    SizeClassInfo::new(8,    8),
    SizeClassInfo::new(16,   16),
    SizeClassInfo::new(32,   16),
    SizeClassInfo::new(64,   64),
    SizeClassInfo::new(128,  64),
    SizeClassInfo::new(256,  64),
    SizeClassInfo::new(512,  64),
    SizeClassInfo::new(1024, 64),
    SizeClassInfo::new(2048, 64),
];

pub const N_SIZE_CLASSES: usize = 9;

/// Retourne l'index de la classe de taille pour `size` octets,
/// ou `None` si `size` dépasse 2 KiB.
#[inline]
pub fn size_class_for(size: usize) -> Option<usize> {
    for (i, sc) in SIZE_CLASSES.iter().enumerate() {
        if size <= sc.size {
            return Some(i);
        }
    }
    None
}

/// Alloue une page de backing pour slab/slub.
///
/// Utilise le buddy dès qu'il est prêt afin d'éviter les doubles allocations
/// physiques entre vmalloc et les caches slab/slub. Le bitmap bootstrap ne sert
/// plus que de secours avant l'initialisation du buddy.
pub(crate) fn alloc_slab_backing_page() -> Result<PhysAddr, AllocError> {
    match crate::memory::physical::allocator::buddy::alloc_page(AllocFlags::NONE) {
        Ok(frame) => Ok(frame.start_address()),
        Err(AllocError::NotInitialized) => {
            let frame = crate::memory::physical::allocator::bitmap::BOOTSTRAP_BITMAP
                .alloc_frame(AllocFlags::NONE)?;
            Ok(frame.start_address())
        }
        Err(e) => Err(e),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HEADER DE SLAB (stocké au début de chaque page de slab)
// ─────────────────────────────────────────────────────────────────────────────

/// Header d'un slab — occupe le début de la page physique du slab.
/// Après ce header, les objets sont disposés en séquence.
///
/// La freelist est une liste chaînée intrusive : chaque objet libre
/// contient un `*mut u8` vers le suivant dans ses premiers octets.
#[repr(C, align(64))]
struct SlabHeader {
    /// Première adresse libre dans la freelist.
    freelist: *mut u8,
    /// Nombre d'objets actifs (alloués).
    inuse: u32,
    /// Nombre total d'objets dans ce slab.
    total: u32,
    /// Lien vers le slab suivant dans la liste partielle/complète.
    next: *mut SlabHeader,
    /// Lien vers le slab précédent.
    prev: *mut SlabHeader,
    /// Index de la classe de taille de ce slab.
    class_idx: u8,
    /// Couleur de cache (offset en octets pour améliorer la répartition).
    color_offset: u8,
    _pad: [u8; 6],
}

/// Taille du header — conservée sous PAGE_SIZE.
const _: () = assert!(mem::size_of::<SlabHeader>() <= 64);

// ─────────────────────────────────────────────────────────────────────────────
// CACHE SLAB
// ─────────────────────────────────────────────────────────────────────────────

/// Statistiques d'un cache slab — sur leur propre cache line.
/// FIX-SLABCACHE-01 : #[repr(C, align(64))] évite le false sharing avec `inner`.
#[repr(C, align(64))]
pub struct SlabCacheStats {
    pub allocs:       AtomicU64,
    pub frees:        AtomicU64,
    pub slabs_created: AtomicU64,
    pub slabs_freed:   AtomicU64,
    pub current_inuse: AtomicUsize,
    pub cache_full:    AtomicU64,
}

impl SlabCacheStats {
    pub const fn new() -> Self {
        SlabCacheStats {
            allocs:        AtomicU64::new(0),
            frees:         AtomicU64::new(0),
            slabs_created: AtomicU64::new(0),
            slabs_freed:   AtomicU64::new(0),
            current_inuse: AtomicUsize::new(0),
            cache_full:    AtomicU64::new(0),
        }
    }
}

/// Newtype forçant son contenu sur une frontière de cache line (64 octets).
/// FIX-SLABCACHE-01 : sépare `inner` et `stats` sur des lignes distinctes.
#[repr(C, align(64))]
struct CacheLineAligned<T>(T);

/// Cache slab pour une classe de taille fixe.
///
/// Maintient trois listes de slabs :
/// - `partial` : slabs avec des objets libres et des objets occupés
/// - `full`    : slabs entièrement occupés
/// - `free`    : slabs entièrement libres (prêts à être restitués au buddy)
///
/// FIX-SLABCACHE-01 : #[repr(C, align(64))] + _cache_line_separator garantissent
/// que `inner` (mutex contesté) et `stats` (atomiques lus depuis d'autres CPUs)
/// sont sur des cache lines distinctes → pas de false sharing SMP.
#[repr(C, align(64))]
pub struct SlabCache {
    info:    SizeClassInfo,
    inner:   Mutex<SlabCacheInner>,
    /// Séparateur : force `stats` sur une cache line dédiée.
    _cache_line_separator: CacheLineAligned<()>,
    pub stats:   SlabCacheStats,
    enabled: AtomicBool,
}

struct SlabCacheInner {
    partial_list: *mut SlabHeader,
    full_list:    *mut SlabHeader,
    free_list:    *mut SlabHeader,
    partial_count: usize,
    full_count:    usize,
    free_count:    usize,
    color_next:    usize, // Couleur actuelle pour le prochain slab
    // Note : plus de vmalloc_ptr — adresse virtuelle dérivée de phys via physmap
}

// SAFETY: SlabCache est protégé par son Mutex interne.
unsafe impl Sync for SlabCache {}
unsafe impl Send for SlabCache {}

impl SlabCache {
    pub const fn new(info: SizeClassInfo) -> Self {
        SlabCache {
            info,
            inner: Mutex::new(SlabCacheInner {
                partial_list:  core::ptr::null_mut(),
                full_list:     core::ptr::null_mut(),
                free_list:     core::ptr::null_mut(),
                partial_count: 0,
                full_count:    0,
                free_count:    0,
                color_next:    0,
            }),
            _cache_line_separator: CacheLineAligned(()),
            stats:   SlabCacheStats::new(),
            enabled: AtomicBool::new(false),
        }
    }

    pub fn enable(&self) {
        self.enabled.store(true, Ordering::Release);
    }

    /// Alloue un objet de cette classe de taille.
    pub fn alloc(&self, _flags: AllocFlags) -> Result<NonNull<u8>, AllocError> {
        if !self.enabled.load(Ordering::Acquire) {
            return Err(AllocError::NotInitialized);
        }
        let mut inner = self.inner.lock();

        // 1. Essayer la liste partielle
        if let Some(ptr) = Self::alloc_from_list(&mut inner.partial_list, &self.info) {
            self.stats.allocs.fetch_add(1, Ordering::Relaxed);
            self.stats.current_inuse.fetch_add(1, Ordering::Relaxed);
            return Ok(ptr);
        }

        // 2. Essayer la liste libre
        if let Some(ptr) = Self::activate_free_slab(&mut inner, &self.info) {
            self.stats.allocs.fetch_add(1, Ordering::Relaxed);
            self.stats.current_inuse.fetch_add(1, Ordering::Relaxed);
            return Ok(ptr);
        }

        // 3. Créer un nouveau slab
        match Self::create_new_slab(&mut inner, &self.info) {
            Ok(ptr) => {
                self.stats.slabs_created.fetch_add(1, Ordering::Relaxed);
                self.stats.allocs.fetch_add(1, Ordering::Relaxed);
                self.stats.current_inuse.fetch_add(1, Ordering::Relaxed);
                Ok(ptr)
            }
            Err(e) => {
                self.stats.cache_full.fetch_add(1, Ordering::Relaxed);
                Err(e)
            }
        }
    }

    /// Libère un objet vers ce cache.
    ///
    /// SAFETY: `ptr` doit avoir été alloué par ce cache et ne pas être
    ///         référencé après cet appel.
    pub unsafe fn free(&self, ptr: NonNull<u8>) {
        let mut inner = self.inner.lock();
        // Retrouver le header du slab (début de la page contenant ptr)
        let page_addr = ptr.as_ptr() as usize & !(PAGE_SIZE - 1);
        let header    = page_addr as *mut SlabHeader;

        // Pousser sur la freelist du slab
        // SAFETY: `header` pointe sur un SlabHeader valide (issu de notre create_new_slab)
        let obj_ptr   = ptr.as_ptr();
        let old_free  = (*header).freelist;
        // Écrire le pointeur vers l'ancien freelist dans les premiers octets de l'objet
        core::ptr::write(obj_ptr as *mut *mut u8, old_free);
        (*header).freelist = obj_ptr;
        (*header).inuse   -= 1;

        let inuse = (*header).inuse;

        let r = &mut *inner;
        if inuse == 0 {
            // SAFETY: header ptr valide (slab actif); listes manipulées sous verrou.
            unsafe {
                list_remove(header, &mut r.partial_list, &mut r.partial_count);
                list_remove(header, &mut r.full_list,    &mut r.full_count);
                list_push_front(header, &mut r.free_list, &mut r.free_count);
            }
            self.stats.current_inuse.fetch_sub(1, Ordering::Relaxed);
            self.stats.frees.fetch_add(1, Ordering::Relaxed);
        } else if inuse + 1 == (*header).total {
            // SAFETY: header ptr valide; cache verrouillé lors de cet appel.
            unsafe {
                list_remove(header, &mut r.full_list, &mut r.full_count);
                list_push_front(header, &mut r.partial_list, &mut r.partial_count);
            }
            self.stats.current_inuse.fetch_sub(1, Ordering::Relaxed);
            self.stats.frees.fetch_add(1, Ordering::Relaxed);
        } else {
            // Reste partiel
            self.stats.current_inuse.fetch_sub(1, Ordering::Relaxed);
            self.stats.frees.fetch_add(1, Ordering::Relaxed);
        }
    }

    // ─────────────── helpers internes ───────────────────────────────────────

    fn alloc_from_list(list: &mut *mut SlabHeader, info: &SizeClassInfo) -> Option<NonNull<u8>> {
        if list.is_null() { return None; }
        // SAFETY: list pointe sur un SlabHeader valide (géré par ce cache)
        let header = unsafe { &mut **list };
        if header.freelist.is_null() { return None; }

        let obj = header.freelist;
        // SAFETY: obj pointe sur un objet libre dont les premiers octets
        //         contiennent le pointeur vers le prochain élément libre
        let next = unsafe { core::ptr::read(obj as *const *mut u8) };
        header.freelist = next;
        header.inuse   += 1;

        if header.inuse == header.total {
            // Slab maintenant plein — sera géré par l'appelant si besoin
        }
        let _ = info;
        NonNull::new(obj)
    }

    fn activate_free_slab(inner: &mut SlabCacheInner, info: &SizeClassInfo) -> Option<NonNull<u8>> {
        if inner.free_list.is_null() { return None; }
        let header = inner.free_list;
        // SAFETY: free_list non-null (vérifié ci-dessus); manipulation de liste sous verrou.
        unsafe {
            list_remove(header, &mut inner.free_list, &mut inner.free_count);
            list_push_front(header, &mut inner.partial_list, &mut inner.partial_count);
        }
        Self::alloc_from_list(&mut inner.partial_list, info)
    }

    /// Crée un nouveau slab en mappant une page physique dans l'espace vmalloc réservé.
    /// Utilise une adresse virtuelle avançante (bump pointer) pour les mappings slab.
    fn create_new_slab(inner: &mut SlabCacheInner, info: &SizeClassInfo) -> Result<NonNull<u8>, AllocError> {
        // Obtenir une page physique via l'allocateur buddy de niveau supérieur.
        // Comme slab.rs est dans physical/allocator/, il n'appelle PAS buddy directement
        // (éviter la dépendance circulaire). On s'appuie sur une fonction fournie
        // par la couche parente via un pointeur de fonction statique.
        let phys = alloc_slab_backing_page()?;

        // Adresse virtuelle = physmap directe (PHYS_MAP_BASE + phys).
        // La physmap couvre toute la RAM physique sans mapping supplémentaire.
        let virt_base = PHYS_MAP_BASE.as_u64() + phys.as_u64();

        // Calculer la couleur de cache
        let color = if info.color_range > 0 {
            (inner.color_next * CACHE_LINE_SIZE) % info.color_range
        } else {
            0
        };
        inner.color_next = (inner.color_next + 1) % (info.color_range / CACHE_LINE_SIZE + 1).max(1);

        // Écrire le header au début de la page (offset 0)
        // SAFETY: virt_base pointe sur une page valide fraîchement allouée et mappée.
        //         Le header est aligné sur 64 octets (= alignement de SlabHeader).
        let header_ptr  = virt_base as *mut SlabHeader;
        let first_obj   = virt_base as usize
            + mem::size_of::<SlabHeader>()
            + align_up_slab(mem::size_of::<SlabHeader>(), info.alignment) - mem::size_of::<SlabHeader>()
            + color;
        let objs_in_page = (PAGE_SIZE - (first_obj - virt_base as usize)) / info.size;
        let objs_in_page = objs_in_page.min(info.objs_per_slab);

        // SAFETY: virt_base = page physique valide et mappée; header_ptr aligné 64 octets.
        unsafe {
            // Construire la freelist en chaîne
            let mut current = first_obj as *mut u8;
            for i in 0..objs_in_page {
                let obj = (first_obj + i * info.size) as *mut u8;
                if i + 1 < objs_in_page {
                    let next = (first_obj + (i + 1) * info.size) as *mut u8;
                    core::ptr::write(obj as *mut *mut u8, next);
                } else {
                    core::ptr::write(obj as *mut *mut u8, core::ptr::null_mut());
                }
                if i == 0 { current = obj; }
            }

            core::ptr::write(header_ptr, SlabHeader {
                freelist:     current,
                inuse:        0,
                total:        objs_in_page as u32,
                next:         core::ptr::null_mut(),
                prev:         core::ptr::null_mut(),
                class_idx:    0,
                color_offset: color as u8,
                _pad:         [0; 6],
            });
        }

        // SAFETY: header_ptr initialisé ci-dessus (page fraîche); liste partielle sous verrou.
        unsafe { list_push_front(header_ptr, &mut inner.partial_list, &mut inner.partial_count); }

        // Allouer le premier objet
        Self::alloc_from_list(&mut inner.partial_list, info).ok_or(AllocError::OutOfMemory)
    }
}

#[inline]
fn align_up_slab(val: usize, align: usize) -> usize {
    (val + align - 1) & !(align - 1)
}

// ─────────────────────────────────────────────────────────────────────────────
// OPÉRATIONS SUR LES LISTES DOUBLEMENT CHAÎNÉES
// ─────────────────────────────────────────────────────────────────────────────

/// Insère `node` en tête de la liste pointée par `head`.
unsafe fn list_push_front(node: *mut SlabHeader, head: &mut *mut SlabHeader, count: &mut usize) {
    (*node).prev = core::ptr::null_mut();
    (*node).next = *head;
    if !(*head).is_null() {
        (*(*head)).prev = node;
    }
    *head  = node;
    *count += 1;
}

/// Retire `node` de la liste pointée par `head` (si présent).
unsafe fn list_remove(node: *mut SlabHeader, head: &mut *mut SlabHeader, count: &mut usize) {
    if node.is_null() { return; }
    let prev = (*node).prev;
    let next = (*node).next;
    if !prev.is_null() { (*prev).next = next; }
    else               { *head        = next; }
    if !next.is_null() { (*next).prev = prev; }
    (*node).prev  = core::ptr::null_mut();
    (*node).next  = core::ptr::null_mut();
    *count = count.saturating_sub(1);
}

// ─────────────────────────────────────────────────────────────────────────────
// FOURNISSEUR DE PAGES POUR SLAB (inversion de dépendance)
// ─────────────────────────────────────────────────────────────────────────────

/// Trait permettant à slab d'obtenir des pages physiques sans dépendre
/// directement du buddy (évite la circularité).
pub trait SlabPageProvider: Sync {
    fn get_page(&self)  -> Result<PhysAddr, AllocError>;
    fn put_page(&self, phys: PhysAddr);
}

/// Implémentation par défaut à base de pointeur statique late-bound.
/// Stocke les deux parties d'un fat pointer (data + vtable) séparément.
pub struct DefaultProvider {
    data:   AtomicUsize,
    vtable: AtomicUsize,
}

impl SlabPageProvider for DefaultProvider {
    fn get_page(&self) -> Result<PhysAddr, AllocError> {
        let data   = self.data.load(Ordering::Acquire);
        let vtable = self.vtable.load(Ordering::Acquire);
        if data == 0 || vtable == 0 { return Err(AllocError::NotInitialized); }
        // SAFETY: data+vtable forment un fat pointer réenregistré par register_slab_page_provider.
        let fat: *const dyn SlabPageProvider = unsafe {
            core::mem::transmute((data as *const (), vtable as *const ()))
        };
        // SAFETY: fat pointer reconstruit depuis data/vtable enregistrés; fat est valide.
        unsafe { (*fat).get_page() }
    }
    fn put_page(&self, phys: PhysAddr) {
        let data   = self.data.load(Ordering::Acquire);
        let vtable = self.vtable.load(Ordering::Acquire);
        if data == 0 || vtable == 0 { return; }
        let fat: *const dyn SlabPageProvider = unsafe {
            core::mem::transmute((data as *const (), vtable as *const ()))
        };
        // SAFETY: fat pointer reconstruit depuis data/vtable enregistrés; fat valide.
        unsafe { (*fat).put_page(phys) }
    }
}

pub static SLAB_PAGE_PROVIDER: DefaultProvider = DefaultProvider {
    data:   AtomicUsize::new(0),
    vtable: AtomicUsize::new(0),
};

/// Enregistre le fournisseur de pages utilisé par slab.
/// Doit être appelé avant tout appel à `alloc()`.
///
/// SAFETY: `provider` doit être une référence statique valide pendant
///         toute la durée de vie du kernel.
pub unsafe fn register_slab_page_provider(provider: *const dyn SlabPageProvider) {
    let (data, vtable): (usize, usize) = core::mem::transmute(provider);
    SLAB_PAGE_PROVIDER.data.store(data, Ordering::SeqCst);
    SLAB_PAGE_PROVIDER.vtable.store(vtable, Ordering::SeqCst);
}

/// Retourne true si un fournisseur de pages a été enregistré (data != 0).
#[inline]
pub fn is_slab_provider_registered() -> bool {
    SLAB_PAGE_PROVIDER.data.load(Ordering::Acquire) != 0
}

// ─────────────────────────────────────────────────────────────────────────────
// TABLE DE CACHES GLOBALE
// ─────────────────────────────────────────────────────────────────────────────

macro_rules! slab_caches_init {
    ($($idx:expr => $size:expr, $align:expr),*) => {
        [$(
            SlabCache::new(SizeClassInfo::new($size, $align)),
        )*]
    }
}

/// Table des caches slab, un par classe de taille.
pub static SLAB_CACHES: [SlabCache; N_SIZE_CLASSES] = slab_caches_init! {
    0 =>    8,  8,
    1 =>   16, 16,
    2 =>   32, 16,
    3 =>   64, 64,
    4 =>  128, 64,
    5 =>  256, 64,
    6 =>  512, 64,
    7 => 1024, 64,
    8 => 2048, 64
};

/// Initialise tous les caches slab.
/// Doit être appelé APRÈS `register_slab_page_provider`.
pub fn init_all() {
    for cache in &SLAB_CACHES {
        cache.enable();
    }
}

/// Alloue un objet de `size` octets via le slab approprié.
pub fn alloc(size: usize, flags: AllocFlags) -> Result<NonNull<u8>, AllocError> {
    let idx = size_class_for(size).ok_or(AllocError::InvalidParams)?;
    SLAB_CACHES[idx].alloc(flags)
}

/// Libère un objet alloué par le slab.
///
/// SAFETY: `ptr` doit avoir été alloué via `alloc()` avec la même `size`,
///         et ne plus être utilisé après cet appel.
pub unsafe fn free(ptr: NonNull<u8>, size: usize) {
    if let Some(idx) = size_class_for(size) {
        SLAB_CACHES[idx].free(ptr);
    }
}
