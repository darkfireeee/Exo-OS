// kernel/src/memory/physical/allocator/bitmap.rs
//
// Bitmap Allocator — allocateur bootstrap simple.
// Utilisé uniquement pendant la phase d'initialisation du kernel,
// avant que le buddy allocator soit opérationnel.
// Couche 0 — aucune dépendance externe.

use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;
use crate::memory::core::{Frame, PhysAddr, AllocFlags, AllocError, PAGE_SIZE};

// ─────────────────────────────────────────────────────────────────────────────
// BITMAP ALLOCATOR
// ─────────────────────────────────────────────────────────────────────────────

/// Taille maximale du bitmap d'amorçage (couvre 512 MiB à 4KiB/page).
const BOOTSTRAP_MAX_FRAMES: usize = 512 * 1024 * 1024 / 4096; // 131072 frames
const BITMAP_WORDS:         usize = BOOTSTRAP_MAX_FRAMES / 64;

/// Bitmap d'amorçage — statique en .bss, jamais désalloué.
///
/// Convention bitmap :
///   bit = 0 → frame LIBRE
///   bit = 1 → frame ALLOUÉ ou RÉSERVÉ
///
/// Initialisé à 0xFF...FF (tout réservé), puis les régions libres
/// sont marquées au fur et à mesure par add_free_region().
pub struct BitmapAllocator {
    inner: Mutex<BitmapInner>,
    initialized: AtomicBool,
}

struct BitmapInner {
    bitmap:       [u64; BITMAP_WORDS],
    phys_start:   u64,
    phys_end:     u64,
    total_frames: usize,
    free_frames:  usize,
    next_hint:    usize,  // Hint de recherche (évite de repartir de 0 à chaque fois)
}

// SAFETY: BitmapAllocator est thread-safe via son spinlock interne.
unsafe impl Sync for BitmapAllocator {}

impl BitmapAllocator {
    pub const fn new() -> Self {
        BitmapAllocator {
            inner: Mutex::new(BitmapInner {
                bitmap:       [u64::MAX; BITMAP_WORDS], // Tout réservé au départ
                phys_start:   0,
                phys_end:     0,
                total_frames: 0,
                free_frames:  0,
                next_hint:    0,
            }),
            initialized: AtomicBool::new(false),
        }
    }

    /// Initialise la plage d'adresses physiques gérée par ce bitmap.
    ///
    /// SAFETY: Single-CPU uniquement (avant init SMP).
    pub unsafe fn init(&self, phys_start: PhysAddr, phys_end: PhysAddr) {
        let mut inner = self.inner.lock();
        inner.phys_start   = phys_start.as_u64();
        inner.phys_end     = phys_end.as_u64();
        inner.total_frames = ((phys_end.as_u64() - phys_start.as_u64()) / PAGE_SIZE as u64) as usize;
        inner.total_frames = inner.total_frames.min(BOOTSTRAP_MAX_FRAMES);
        inner.free_frames  = 0;
        // Initialiser tout à "réservé"
        for w in &mut inner.bitmap {
            *w = u64::MAX;
        }
        drop(inner);
        self.initialized.store(true, Ordering::Release);
    }

    /// Marque une plage physique comme libre (disponible à l'allocation).
    ///
    /// SAFETY: Single-CPU uniquement.
    pub unsafe fn add_free_region(&self, start: PhysAddr, end: PhysAddr) {
        debug_assert!(self.initialized.load(Ordering::Acquire));
        let mut inner = self.inner.lock();
        let base = inner.phys_start;

        let first_pfn = ((start.as_u64().saturating_sub(base)) / PAGE_SIZE as u64) as usize;
        let last_pfn  = ((end.as_u64().saturating_sub(base))   / PAGE_SIZE as u64) as usize;
        let last_pfn  = last_pfn.min(BOOTSTRAP_MAX_FRAMES);

        for pfn in first_pfn..last_pfn {
            let word = pfn / 64;
            let bit  = pfn % 64;
            if word < BITMAP_WORDS {
                let was_set = (inner.bitmap[word] >> bit) & 1;
                if was_set == 1 {
                    inner.bitmap[word] &= !(1u64 << bit);
                    inner.free_frames += 1;
                }
            }
        }
    }

    /// Alloue un seul frame physique (ordre 0 uniquement — bootstrap uniquement).
    pub fn alloc_frame(&self, _flags: AllocFlags) -> Result<Frame, AllocError> {
        if !self.initialized.load(Ordering::Acquire) {
            return Err(AllocError::NotInitialized);
        }

        let mut inner = self.inner.lock();
        if inner.free_frames == 0 {
            return Err(AllocError::OutOfMemory);
        }

        // Recherche à partir du hint (évite O(n) systématique)
        let start_word = inner.next_hint / 64;
        let nwords     = BITMAP_WORDS;

        for i in 0..nwords {
            let word_idx   = (start_word + i) % nwords;
            let word_val   = inner.bitmap[word_idx];
            if word_val == u64::MAX {
                continue; // Tous alloués dans ce mot
            }
            // Trouver le premier bit à 0 (frame libre)
            let bit = (!word_val).trailing_zeros() as usize;
            let pfn = word_idx * 64 + bit;
            if pfn >= inner.total_frames {
                continue;
            }
            // Marquer comme alloué
            inner.bitmap[word_idx] |= 1u64 << bit;
            inner.free_frames -= 1;
            inner.next_hint = pfn + 1;

            let phys = PhysAddr::new(inner.phys_start + pfn as u64 * PAGE_SIZE as u64);
            return Ok(Frame::containing(phys));
        }

        Err(AllocError::OutOfMemory)
    }

    /// Libère un frame précédemment alloué.
    pub fn free_frame(&self, frame: Frame) {
        if !self.initialized.load(Ordering::Acquire) { return; }

        let mut inner = self.inner.lock();
        let phys = frame.start_address();
        if phys.as_u64() < inner.phys_start || phys.as_u64() >= inner.phys_end {
            return;
        }
        let pfn = ((phys.as_u64() - inner.phys_start) / PAGE_SIZE as u64) as usize;
        if pfn >= BOOTSTRAP_MAX_FRAMES { return; }
        let word = pfn / 64;
        let bit  = pfn % 64;
        debug_assert!((inner.bitmap[word] >> bit) & 1 == 1,
            "free_frame: frame déjà libre (double-free)");
        inner.bitmap[word] &= !(1u64 << bit);
        inner.free_frames += 1;
        if pfn < inner.next_hint {
            inner.next_hint = pfn;
        }
    }

    /// Retourne le nombre de frames libres.
    pub fn free_frames(&self) -> usize {
        self.inner.lock().free_frames
    }

    /// Vérifie si un frame est libre.
    pub fn is_free(&self, frame: Frame) -> bool {
        let inner  = self.inner.lock();
        let phys   = frame.start_address();
        if phys.as_u64() < inner.phys_start || phys.as_u64() >= inner.phys_end {
            return false;
        }
        let pfn = ((phys.as_u64() - inner.phys_start) / PAGE_SIZE as u64) as usize;
        if pfn >= BOOTSTRAP_MAX_FRAMES { return false; }
        let word = pfn / 64;
        let bit  = pfn % 64;
        (inner.bitmap[word] >> bit) & 1 == 0
    }
}

/// Bitmap allocator global d'amorçage.
/// Utilisé pendant la phase de boot avant que le buddy soit opérationnel.
pub static BOOTSTRAP_BITMAP: BitmapAllocator = BitmapAllocator::new();
