// kernel/src/memory/heap/allocator/hybrid.rs
//
// Allocateur heap hybride — dispatch entre SLUB (<= 2 KiB) et large (> 2 KiB).
// Interface unique pour le heap kernel.
// Couche 0 — aucune dépendance externe sauf `spin`.

use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::size_classes::{heap_size_class_for, HEAP_LARGE_THRESHOLD, HEAP_SIZE_CLASSES};
use crate::memory::core::{AllocError, AllocFlags};
use crate::memory::physical::allocator::slub::SLUB_CACHES;

// ─────────────────────────────────────────────────────────────────────────────
// STATISTIQUES
// ─────────────────────────────────────────────────────────────────────────────

pub struct HybridAllocStats {
    pub small_allocs: AtomicU64,
    pub small_frees: AtomicU64,
    pub large_allocs: AtomicU64,
    pub large_frees: AtomicU64,
    pub oom_count: AtomicU64,
    pub current_inuse: AtomicU64,
}

impl HybridAllocStats {
    pub const fn new() -> Self {
        HybridAllocStats {
            small_allocs: AtomicU64::new(0),
            small_frees: AtomicU64::new(0),
            large_allocs: AtomicU64::new(0),
            large_frees: AtomicU64::new(0),
            oom_count: AtomicU64::new(0),
            current_inuse: AtomicU64::new(0),
        }
    }
}

pub static HEAP_STATS: HybridAllocStats = HybridAllocStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// ALLOCATEUR HYBRIDE
// ─────────────────────────────────────────────────────────────────────────────

/// Mode d'allocateur utilisé (SLUB ou Large).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocBackend {
    Slub,
    Large,
}

static HYBRID_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn init() {
    HYBRID_ENABLED.store(true, Ordering::Release);
}

/// Retourne vrai quand l'allocateur heap kernel accepte les allocations.
#[inline]
pub fn is_heap_ready() -> bool {
    HYBRID_ENABLED.load(Ordering::Acquire)
}

// ─────────────────────────────────────────────────────────────────────────────
// DIAG REDZONE (KASAN-lite) — détecte les débordements de buffer heap.
// Chaque allocation réserve REDZONE octets de canari APRÈS la zone utilisable
// `size`. Un débordement écrit dans le canari : détecté au free (la taille de
// l'alloc fautive identifie la structure), et souvent absorbé (le boot passe).
// ─────────────────────────────────────────────────────────────────────────────
const REDZONE: usize = 32;
const REDZ_BYTE: u8 = 0xA5;

#[inline]
unsafe fn redzone_write(ptr: *mut u8, user_size: usize) {
    let mut i = 0usize;
    while i < REDZONE {
        core::ptr::write(ptr.add(user_size + i), REDZ_BYTE);
        i += 1;
    }
}

/// Vérifie le canari ; retourne false si débordé. Logue (E9, capé) la taille
/// de l'alloc fautive pour identifier la structure qui déborde.
#[inline]
unsafe fn redzone_check(ptr: *const u8, user_size: usize) -> bool {
    let mut ok = true;
    let mut i = 0usize;
    while i < REDZONE {
        if core::ptr::read(ptr.add(user_size + i)) != REDZ_BYTE {
            ok = false;
            break;
        }
        i += 1;
    }
    if !ok {
        use core::sync::atomic::AtomicUsize;
        static N: AtomicUsize = AtomicUsize::new(0);
        if N.fetch_add(1, Ordering::Relaxed) < 8 {
            let out = crate::arch::x86_64::terminal::debug_write;
            out(b"\n<REDZ overflow user_size=");
            let mut s = user_size as u64;
            let mut buf = [0u8; 20];
            let mut k = buf.len();
            if s == 0 {
                k -= 1;
                buf[k] = b'0';
            }
            while s != 0 && k > 0 {
                k -= 1;
                buf[k] = b'0' + (s % 10) as u8;
                s /= 10;
            }
            out(&buf[k..]);
            out(b">");
        }
    }
    ok
}

/// Alloue `size` octets avec `align` minimum.
///
/// - Si size <= 2048 : utilise SLUB.
/// - Sinon : utilise le large allocator (vmalloc).
pub fn alloc(size: usize, _align: usize, flags: AllocFlags) -> Result<NonNull<u8>, AllocError> {
    if !HYBRID_ENABLED.load(Ordering::Acquire) {
        return Err(AllocError::NotInitialized);
    }

    let user_size = if size == 0 { 8 } else { size };
    // DIAG REDZONE : réserver REDZONE octets de canari après la zone utilisable.
    let real_size = user_size.saturating_add(REDZONE);

    if real_size <= HEAP_LARGE_THRESHOLD {
        // Petite allocation via SLUB
        let sc_entry = heap_size_class_for(real_size).ok_or(AllocError::InvalidParams)?;
        let slab_idx = HEAP_SIZE_CLASSES[sc_entry].slab_idx;
        match SLUB_CACHES[slab_idx].alloc(flags) {
            Ok(ptr) => {
                unsafe { redzone_write(ptr.as_ptr(), user_size) };
                HEAP_STATS.small_allocs.fetch_add(1, Ordering::Relaxed);
                HEAP_STATS
                    .current_inuse
                    .fetch_add(real_size as u64, Ordering::Relaxed);
                Ok(ptr)
            }
            Err(_e) => {
                // Fallback de robustesse: si SLUB n'est pas encore prêt ou échoue
                // ponctuellement, tenter l'allocateur large pour éviter un panic
                // précoce durant le boot.
                match crate::memory::heap::large::vmalloc::kalloc(real_size, flags) {
                    Ok(ptr) => {
                        unsafe { redzone_write(ptr.as_ptr(), user_size) };
                        HEAP_STATS.large_allocs.fetch_add(1, Ordering::Relaxed);
                        HEAP_STATS
                            .current_inuse
                            .fetch_add(real_size as u64, Ordering::Relaxed);
                        Ok(ptr)
                    }
                    Err(e2) => {
                        HEAP_STATS.oom_count.fetch_add(1, Ordering::Relaxed);
                        Err(e2)
                    }
                }
            }
        }
    } else {
        // Grande allocation via le large allocator
        match crate::memory::heap::large::vmalloc::kalloc(real_size, flags) {
            Ok(ptr) => {
                unsafe { redzone_write(ptr.as_ptr(), user_size) };
                HEAP_STATS.large_allocs.fetch_add(1, Ordering::Relaxed);
                HEAP_STATS
                    .current_inuse
                    .fetch_add(real_size as u64, Ordering::Relaxed);
                Ok(ptr)
            }
            Err(e) => {
                HEAP_STATS.oom_count.fetch_add(1, Ordering::Relaxed);
                Err(e)
            }
        }
    }
}

/// Libère un pointeur alloué par `alloc`.
///
/// SAFETY: `ptr` doit avoir été alloué par `alloc()` avec la même `size`.
///         Ne plus être utilisé après cet appel.
pub unsafe fn free(ptr: NonNull<u8>, size: usize) {
    let user_size = if size == 0 { 8 } else { size };
    // DIAG REDZONE : vérifier le canari avant de libérer (détecte un débordement
    // d'une allocation adjacente OU de celle-ci). La taille logguée identifie la
    // structure fautive.
    let _ = redzone_check(ptr.as_ptr(), user_size);
    let real_size = user_size.saturating_add(REDZONE);

    // Si le pointeur correspond à un bloc vmalloc (incluant fallback small->large),
    // il doit être libéré via kfree indépendamment de la taille demandée.
    if let Some(usable) = crate::memory::heap::large::vmalloc::kalloc_usable_size(ptr) {
        crate::memory::heap::large::vmalloc::kfree(ptr, usable);
        HEAP_STATS.large_frees.fetch_add(1, Ordering::Relaxed);
        HEAP_STATS
            .current_inuse
            .fetch_sub(usable as u64, Ordering::Relaxed);
        return;
    }

    if real_size <= HEAP_LARGE_THRESHOLD {
        let sc_entry = match heap_size_class_for(real_size) {
            Some(i) => i,
            None => return,
        };
        let slab_idx = HEAP_SIZE_CLASSES[sc_entry].slab_idx;
        SLUB_CACHES[slab_idx].free(ptr);
        HEAP_STATS.small_frees.fetch_add(1, Ordering::Relaxed);
        // Décrémenter de façon simple; la stat peut brièvement sous-estimer sous
        // contention mais jamais provoquer de comportement incorrect.
        HEAP_STATS
            .current_inuse
            .fetch_sub(real_size as u64, Ordering::Relaxed);
    } else {
        crate::memory::heap::large::vmalloc::kfree(ptr, real_size);
        HEAP_STATS.large_frees.fetch_add(1, Ordering::Relaxed);
        HEAP_STATS
            .current_inuse
            .fetch_sub(real_size as u64, Ordering::Relaxed);
    }
}
