// kernel/src/memory/physical/allocator/slab.rs
//
// Slab allocator — fixed-size small object allocations.
// Inspired by Bonwick's slab allocator (1994) but adapted for no_std.
// Layer 0 — no external dependencies except `spin`.

use core::mem;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

use crate::memory::core::layout::PHYS_MAP_BASE;
use crate::memory::core::{AllocError, AllocFlags, PhysAddr, CACHE_LINE_SIZE, PAGE_SIZE};

// ─────────────────────────────────────────────────────────────────────────────
// SLAB SIZE CLASSES
// ─────────────────────────────────────────────────────────────────────────────

/// Size classes supported by the slab allocator.
/// Each class manages fixed-size objects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeClassInfo {
    pub size: usize,          // Object size in bytes
    pub alignment: usize,     // Required alignment
    pub objs_per_slab: usize, // Number of objects per slab page
    pub color_range: usize,   // Number of cache colors
}

impl SizeClassInfo {
    pub const fn new(size: usize, alignment: usize) -> Self {
        let usable = PAGE_SIZE - mem::size_of::<SlabHeader>();
        let objs = usable / size;
        let waste = usable - objs * size;
        SizeClassInfo {
            size,
            alignment,
            objs_per_slab: objs,
            color_range: waste,
        }
    }
}

/// Standard size classes (8 B -> 2 KiB).
pub const SIZE_CLASSES: &[SizeClassInfo] = &[
    SizeClassInfo::new(8, 8),
    SizeClassInfo::new(16, 16),
    SizeClassInfo::new(32, 16),
    SizeClassInfo::new(64, 64),
    SizeClassInfo::new(128, 64),
    SizeClassInfo::new(256, 64),
    SizeClassInfo::new(512, 64),
    SizeClassInfo::new(1024, 64),
    SizeClassInfo::new(2048, 64),
];

pub const N_SIZE_CLASSES: usize = 9;

/// Returns size class index for `size` bytes,
/// or `None` if `size` exceeds 2 KiB.
#[inline]
pub fn size_class_for(size: usize) -> Option<usize> {
    for (i, sc) in SIZE_CLASSES.iter().enumerate() {
        if size <= sc.size {
            return Some(i);
        }
    }
    None
}

/// Allocates a backing page for slab/slub.
///
/// Uses buddy as soon as it is ready to avoid double physical
/// allocations between vmalloc and slab/slub caches. Bootstrap bitmap
/// only serves as fallback prior to buddy initialization.
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
// SLAB HEADER (stored at the beginning of each slab page)
// ─────────────────────────────────────────────────────────────────────────────

/// Slab header — occupies the beginning of the slab physical page.
/// Following this header, objects are laid out sequentially.
///
/// The freelist is an intrusive linked list: each free object
/// contains a `*mut u8` pointing to the next in its first bytes.
#[repr(C, align(64))]
struct SlabHeader {
    /// First free address in freelist.
    freelist: *mut u8,
    /// Number of active (allocated) objects.
    inuse: u32,
    /// Total number of objects in this slab.
    total: u32,
    /// Link to next slab in partial/full list.
    next: *mut SlabHeader,
    /// Link to previous slab.
    prev: *mut SlabHeader,
    /// Size class index of this slab.
    class_idx: u8,
    /// Cache color (offset in bytes to improve distribution).
    color_offset: u8,
    _pad: [u8; 6],
}

/// Header size — kept under PAGE_SIZE.
const _: () = assert!(mem::size_of::<SlabHeader>() <= 64);

// ─────────────────────────────────────────────────────────────────────────────
// SLAB CACHE
// ─────────────────────────────────────────────────────────────────────────────

/// Slab cache statistics — on their own cache line.
/// FIX-SLABCACHE-01: #[repr(C, align(64))] avoids false sharing with `inner`.
#[repr(C, align(64))]
pub struct SlabCacheStats {
    pub allocs: AtomicU64,
    pub frees: AtomicU64,
    pub slabs_created: AtomicU64,
    pub slabs_freed: AtomicU64,
    pub current_inuse: AtomicUsize,
    pub cache_full: AtomicU64,
}

impl Default for SlabCacheStats {
    fn default() -> Self {
        Self::new()
    }
}

impl SlabCacheStats {
    pub const fn new() -> Self {
        SlabCacheStats {
            allocs: AtomicU64::new(0),
            frees: AtomicU64::new(0),
            slabs_created: AtomicU64::new(0),
            slabs_freed: AtomicU64::new(0),
            current_inuse: AtomicUsize::new(0),
            cache_full: AtomicU64::new(0),
        }
    }
}

/// Newtype forcing its content onto a cache line boundary (64 bytes).
/// FIX-SLABCACHE-01: separates `inner` and `stats` onto distinct lines.
#[repr(C, align(64))]
struct CacheLineAligned<T>(T);

/// Slab cache for a fixed size class.
///
/// Maintains three slab lists:
/// - `partial`: slabs with both free and occupied objects
/// - `full`   : fully occupied slabs
/// - `free`   : completely free slabs (ready to be returned to buddy)
///
/// FIX-SLABCACHE-01: #[repr(C, align(64))] + _cache_line_separator ensure
/// that `inner` (contended mutex) and `stats` (atomics read from other CPUs)
/// are on distinct cache lines -> no SMP false sharing.
#[repr(C, align(64))]
pub struct SlabCache {
    info: SizeClassInfo,
    inner: Mutex<SlabCacheInner>,
    /// Separator: forces `stats` onto a dedicated cache line.
    _cache_line_separator: CacheLineAligned<()>,
    pub stats: SlabCacheStats,
    enabled: AtomicBool,
}

struct SlabCacheInner {
    partial_list: *mut SlabHeader,
    full_list: *mut SlabHeader,
    free_list: *mut SlabHeader,
    partial_count: usize,
    full_count: usize,
    free_count: usize,
    color_next: usize, // Current color for next slab
                       // Note: no more vmalloc_ptr — virtual address derived from phys via physmap
}

// SAFETY: SlabCache is protected by its internal Mutex.
unsafe impl Sync for SlabCache {}
unsafe impl Send for SlabCache {}

impl SlabCache {
    pub const fn new(info: SizeClassInfo) -> Self {
        SlabCache {
            info,
            inner: Mutex::new(SlabCacheInner {
                partial_list: core::ptr::null_mut(),
                full_list: core::ptr::null_mut(),
                free_list: core::ptr::null_mut(),
                partial_count: 0,
                full_count: 0,
                free_count: 0,
                color_next: 0,
            }),
            _cache_line_separator: CacheLineAligned(()),
            stats: SlabCacheStats::new(),
            enabled: AtomicBool::new(false),
        }
    }

    pub fn enable(&self) {
        self.enabled.store(true, Ordering::Release);
    }

    /// Allocates an object of this size class.
    pub fn alloc(&self, _flags: AllocFlags) -> Result<NonNull<u8>, AllocError> {
        if !self.enabled.load(Ordering::Acquire) {
            return Err(AllocError::NotInitialized);
        }
        let mut inner = self.inner.lock();

        // 1. Try partial list
        if let Some(ptr) = Self::alloc_from_list(&mut inner.partial_list, &self.info) {
            self.stats.allocs.fetch_add(1, Ordering::Relaxed);
            self.stats.current_inuse.fetch_add(1, Ordering::Relaxed);
            return Ok(ptr);
        }

        // 2. Try free list
        if let Some(ptr) = Self::activate_free_slab(&mut inner, &self.info) {
            self.stats.allocs.fetch_add(1, Ordering::Relaxed);
            self.stats.current_inuse.fetch_add(1, Ordering::Relaxed);
            return Ok(ptr);
        }

        // 3. Create new slab
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

    /// Frees an object back to this cache.
    ///
    /// # Safety
    /// `ptr` must have been allocated by this cache and not be
    /// referenced after this call.
    pub unsafe fn free(&self, ptr: NonNull<u8>) {
        let mut inner = self.inner.lock();
        // Retrieve slab header (beginning of page containing ptr)
        let page_addr = ptr.as_ptr() as usize & !(PAGE_SIZE - 1);
        let header = page_addr as *mut SlabHeader;

        // Push onto slab freelist
        // SAFETY: `header` points to a valid SlabHeader (from our create_new_slab)
        let obj_ptr = ptr.as_ptr();
        let old_free = (*header).freelist;
        // Write pointer to former freelist into first bytes of object
        core::ptr::write(obj_ptr as *mut *mut u8, old_free);
        (*header).freelist = obj_ptr;
        (*header).inuse -= 1;

        let inuse = (*header).inuse;

        let r = &mut *inner;
        if inuse == 0 {
            // SAFETY: valid header ptr (active slab); lists manipulated under lock.
            unsafe {
                list_remove(header, &mut r.partial_list, &mut r.partial_count);
                list_remove(header, &mut r.full_list, &mut r.full_count);
                list_push_front(header, &mut r.free_list, &mut r.free_count);
            }
            self.stats.current_inuse.fetch_sub(1, Ordering::Relaxed);
            self.stats.frees.fetch_add(1, Ordering::Relaxed);
        } else if inuse + 1 == (*header).total {
            // SAFETY: valid header ptr; cache locked during this call.
            unsafe {
                list_remove(header, &mut r.full_list, &mut r.full_count);
                list_push_front(header, &mut r.partial_list, &mut r.partial_count);
            }
            self.stats.current_inuse.fetch_sub(1, Ordering::Relaxed);
            self.stats.frees.fetch_add(1, Ordering::Relaxed);
        } else {
            // Remains partial
            self.stats.current_inuse.fetch_sub(1, Ordering::Relaxed);
            self.stats.frees.fetch_add(1, Ordering::Relaxed);
        }
    }

    // ─────────────── internal helpers ───────────────────────────────────────

    fn alloc_from_list(list: &mut *mut SlabHeader, info: &SizeClassInfo) -> Option<NonNull<u8>> {
        if list.is_null() {
            return None;
        }
        // SAFETY: list points to a valid SlabHeader (managed by this cache)
        let header = unsafe { &mut **list };
        if header.freelist.is_null() {
            return None;
        }

        let obj = header.freelist;
        // SAFETY: obj points to a free object whose first bytes
        //         contain the pointer to the next free element
        let next = unsafe { core::ptr::read(obj as *const *mut u8) };
        header.freelist = next;
        header.inuse += 1;

        if header.inuse == header.total {
            // Slab now full — will be handled by caller if needed
        }
        let _ = info;
        NonNull::new(obj)
    }

    fn activate_free_slab(inner: &mut SlabCacheInner, info: &SizeClassInfo) -> Option<NonNull<u8>> {
        if inner.free_list.is_null() {
            return None;
        }
        let header = inner.free_list;
        // SAFETY: free_list non-null (verified above); list manipulation under lock.
        unsafe {
            list_remove(header, &mut inner.free_list, &mut inner.free_count);
            list_push_front(header, &mut inner.partial_list, &mut inner.partial_count);
        }
        Self::alloc_from_list(&mut inner.partial_list, info)
    }

    /// Creates a new slab by mapping a physical page into reserved vmalloc space.
    /// Uses a bump pointer for slab mappings.
    fn create_new_slab(
        inner: &mut SlabCacheInner,
        info: &SizeClassInfo,
    ) -> Result<NonNull<u8>, AllocError> {
        // Obtain a physical page via higher-level buddy allocator.
        // As slab.rs is in physical/allocator/, it does NOT call buddy directly
        // (to avoid circular dependency). We rely on a function provided
        // by parent layer via a static function pointer.
        let phys = alloc_slab_backing_page()?;

        // Virtual address = direct physmap (PHYS_MAP_BASE + phys).
        // Physmap covers all physical RAM without additional mapping.
        let virt_base = PHYS_MAP_BASE.as_u64() + phys.as_u64();

        // Compute cache color
        let color = if info.color_range > 0 {
            (inner.color_next * CACHE_LINE_SIZE) % info.color_range
        } else {
            0
        };
        inner.color_next = (inner.color_next + 1) % (info.color_range / CACHE_LINE_SIZE + 1).max(1);

        // Write header at start of page (offset 0)
        // SAFETY: virt_base points to a valid freshly allocated and mapped page.
        //         Header is aligned to 64 bytes (= SlabHeader alignment).
        let header_ptr = virt_base as *mut SlabHeader;
        let first_obj = virt_base as usize
            + mem::size_of::<SlabHeader>()
            + align_up_slab(mem::size_of::<SlabHeader>(), info.alignment)
            - mem::size_of::<SlabHeader>()
            + color;
        let objs_in_page = (PAGE_SIZE - (first_obj - virt_base as usize)) / info.size;
        let objs_in_page = objs_in_page.min(info.objs_per_slab);

        // SAFETY: virt_base = valid mapped physical page; header_ptr 64-byte aligned.
        unsafe {
            // Build linked freelist
            let mut current = first_obj as *mut u8;
            for i in 0..objs_in_page {
                let obj = (first_obj + i * info.size) as *mut u8;
                if i + 1 < objs_in_page {
                    let next = (first_obj + (i + 1) * info.size) as *mut u8;
                    core::ptr::write(obj as *mut *mut u8, next);
                } else {
                    core::ptr::write(obj as *mut *mut u8, core::ptr::null_mut());
                }
                if i == 0 {
                    current = obj;
                }
            }

            core::ptr::write(
                header_ptr,
                SlabHeader {
                    freelist: current,
                    inuse: 0,
                    total: objs_in_page as u32,
                    next: core::ptr::null_mut(),
                    prev: core::ptr::null_mut(),
                    class_idx: 0,
                    color_offset: color as u8,
                    _pad: [0; 6],
                },
            );
        }

        // SAFETY: header_ptr initialized above (fresh page); partial list under lock.
        unsafe {
            list_push_front(
                header_ptr,
                &mut inner.partial_list,
                &mut inner.partial_count,
            );
        }

        // Allocate first object
        Self::alloc_from_list(&mut inner.partial_list, info).ok_or(AllocError::OutOfMemory)
    }
}

#[inline]
fn align_up_slab(val: usize, align: usize) -> usize {
    (val + align - 1) & !(align - 1)
}

// ─────────────────────────────────────────────────────────────────────────────
// DOUBLY LINKED LIST OPERATIONS
// ─────────────────────────────────────────────────────────────────────────────

/// Inserts `node` at head of list pointed by `head`.
unsafe fn list_push_front(node: *mut SlabHeader, head: &mut *mut SlabHeader, count: &mut usize) {
    (*node).prev = core::ptr::null_mut();
    (*node).next = *head;
    if !(*head).is_null() {
        (*(*head)).prev = node;
    }
    *head = node;
    *count += 1;
}

/// Removes `node` from list pointed by `head` (if present).
unsafe fn list_remove(node: *mut SlabHeader, head: &mut *mut SlabHeader, count: &mut usize) {
    if node.is_null() {
        return;
    }
    let prev = (*node).prev;
    let next = (*node).next;
    if !prev.is_null() {
        (*prev).next = next;
    } else {
        *head = next;
    }
    if !next.is_null() {
        (*next).prev = prev;
    }
    (*node).prev = core::ptr::null_mut();
    (*node).next = core::ptr::null_mut();
    *count = count.saturating_sub(1);
}

// ─────────────────────────────────────────────────────────────────────────────
// PAGE PROVIDER FOR SLAB (dependency inversion)
// ─────────────────────────────────────────────────────────────────────────────

/// Trait allowing slab to obtain physical pages without depending
/// directly on buddy (avoids circularity).
pub trait SlabPageProvider: Sync {
    fn get_page(&self) -> Result<PhysAddr, AllocError>;
    fn put_page(&self, phys: PhysAddr);
}

/// Default implementation based on late-bound static pointer.
/// Stores both parts of a fat pointer (data + vtable) separately.
pub struct DefaultProvider {
    data: AtomicUsize,
    vtable: AtomicUsize,
}

impl SlabPageProvider for DefaultProvider {
    fn get_page(&self) -> Result<PhysAddr, AllocError> {
        let data = self.data.load(Ordering::Acquire);
        let vtable = self.vtable.load(Ordering::Acquire);
        if data == 0 || vtable == 0 {
            return Err(AllocError::NotInitialized);
        }
        // SAFETY: data+vtable form a fat pointer registered via register_slab_page_provider.
        let fat: *const dyn SlabPageProvider =
            unsafe { core::mem::transmute((data as *const (), vtable as *const ())) };
        // SAFETY: fat pointer reconstructed from stored data/vtable; fat is valid.
        unsafe { (*fat).get_page() }
    }
    fn put_page(&self, phys: PhysAddr) {
        let data = self.data.load(Ordering::Acquire);
        let vtable = self.vtable.load(Ordering::Acquire);
        if data == 0 || vtable == 0 {
            return;
        }
        let fat: *const dyn SlabPageProvider =
            unsafe { core::mem::transmute((data as *const (), vtable as *const ())) };
        // SAFETY: fat pointer reconstructed from stored data/vtable; fat is valid.
        unsafe { (*fat).put_page(phys) }
    }
}

pub static SLAB_PAGE_PROVIDER: DefaultProvider = DefaultProvider {
    data: AtomicUsize::new(0),
    vtable: AtomicUsize::new(0),
};

/// Registers page provider used by slab.
/// Must be called before any call to `alloc()`.
///
/// # Safety
/// `provider` must be a valid static reference for the entire
/// kernel lifetime.
pub unsafe fn register_slab_page_provider(provider: *const dyn SlabPageProvider) {
    let (data, vtable): (usize, usize) = core::mem::transmute(provider);
    SLAB_PAGE_PROVIDER.data.store(data, Ordering::SeqCst);
    SLAB_PAGE_PROVIDER.vtable.store(vtable, Ordering::SeqCst);
}

/// Returns true if a page provider has been registered (data != 0).
#[inline]
pub fn is_slab_provider_registered() -> bool {
    SLAB_PAGE_PROVIDER.data.load(Ordering::Acquire) != 0
}

// ─────────────────────────────────────────────────────────────────────────────
// GLOBAL CACHE TABLE
// ─────────────────────────────────────────────────────────────────────────────

macro_rules! slab_caches_init {
    ($($idx:expr => $size:expr, $align:expr),*) => {
        [$(
            SlabCache::new(SizeClassInfo::new($size, $align)),
        )*]
    }
}

/// Slab cache table, one per size class.
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

/// Initializes all slab caches.
/// Must be called AFTER `register_slab_page_provider`.
pub fn init_all() {
    for cache in &SLAB_CACHES {
        cache.enable();
    }
}

/// Allocates an object of `size` bytes via appropriate slab.
pub fn alloc(size: usize, flags: AllocFlags) -> Result<NonNull<u8>, AllocError> {
    let idx = size_class_for(size).ok_or(AllocError::InvalidParams)?;
    SLAB_CACHES[idx].alloc(flags)
}

/// Frees an object allocated by slab.
///
/// # Safety
/// `ptr` must have been allocated via `alloc()` with the same `size`,
/// and must no longer be used after this call.
pub unsafe fn free(ptr: NonNull<u8>, size: usize) {
    if let Some(idx) = size_class_for(size) {
        SLAB_CACHES[idx].free(ptr);
    }
}
