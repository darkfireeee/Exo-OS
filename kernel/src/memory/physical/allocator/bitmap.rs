// kernel/src/memory/physical/allocator/bitmap.rs
//
// Bitmap Allocator — simple bootstrap allocator.
// Used only during the kernel initialization phase,
// before the buddy allocator is operational.
// Layer 0 — no external dependencies.

use crate::memory::core::{AllocError, AllocFlags, Frame, PhysAddr, PAGE_SIZE};
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
// BITMAP ALLOCATOR
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum bootstrap bitmap size (covers 512 MiB at 4KiB/page).
const BOOTSTRAP_MAX_FRAMES: usize = 512 * 1024 * 1024 / 4096; // 131072 frames
const BITMAP_WORDS: usize = BOOTSTRAP_MAX_FRAMES / 64;

/// Bootstrap bitmap — static in .bss, never deallocated.
///
/// Bitmap convention:
///   bit = 0 -> frame FREE
///   bit = 1 -> frame ALLOCATED or RESERVED
///
/// Initialized to 0xFF...FF (all reserved), then free regions
/// are progressively marked by add_free_region().
pub struct BitmapAllocator {
    inner: Mutex<BitmapInner>,
    initialized: AtomicBool,
}

struct BitmapInner {
    bitmap: [u64; BITMAP_WORDS],
    phys_start: u64,
    phys_end: u64,
    total_frames: usize,
    free_frames: usize,
    next_hint: usize, // Search hint (avoids restarting from 0 each time)
}

// SAFETY: BitmapAllocator is thread-safe via its internal spinlock.
unsafe impl Sync for BitmapAllocator {}

impl Default for BitmapAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl BitmapAllocator {
    pub const fn new() -> Self {
        BitmapAllocator {
            inner: Mutex::new(BitmapInner {
                bitmap: [u64::MAX; BITMAP_WORDS], // All reserved initially
                phys_start: 0,
                phys_end: 0,
                total_frames: 0,
                free_frames: 0,
                next_hint: 0,
            }),
            initialized: AtomicBool::new(false),
        }
    }

    /// Initializes the physical address range managed by this bitmap.
    ///
    /// # Safety
    /// Single-CPU only (before SMP init).
    pub unsafe fn init(&self, phys_start: PhysAddr, phys_end: PhysAddr) {
        let mut inner = self.inner.lock();
        inner.phys_start = phys_start.as_u64();
        inner.phys_end = phys_end.as_u64();
        inner.total_frames =
            ((phys_end.as_u64() - phys_start.as_u64()) / PAGE_SIZE as u64) as usize;
        inner.total_frames = inner.total_frames.min(BOOTSTRAP_MAX_FRAMES);
        inner.free_frames = 0;
        // Initialize all to "reserved"
        for w in &mut inner.bitmap {
            *w = u64::MAX;
        }
        drop(inner);
        self.initialized.store(true, Ordering::Release);
    }

    /// Marks a physical range as free (available for allocation).
    ///
    /// # Safety
    /// Single-CPU only.
    pub unsafe fn add_free_region(&self, start: PhysAddr, end: PhysAddr) {
        debug_assert!(self.initialized.load(Ordering::Acquire));
        let mut inner = self.inner.lock();
        let base = inner.phys_start;

        let first_pfn = ((start.as_u64().saturating_sub(base)) / PAGE_SIZE as u64) as usize;
        let last_pfn = ((end.as_u64().saturating_sub(base)) / PAGE_SIZE as u64) as usize;
        let last_pfn = last_pfn.min(BOOTSTRAP_MAX_FRAMES);

        for pfn in first_pfn..last_pfn {
            let word = pfn / 64;
            let bit = pfn % 64;
            if word < BITMAP_WORDS {
                let was_set = (inner.bitmap[word] >> bit) & 1;
                if was_set == 1 {
                    inner.bitmap[word] &= !(1u64 << bit);
                    inner.free_frames += 1;
                }
            }
        }
    }

    /// Allocates a single physical frame (order 0 only — bootstrap only).
    pub fn alloc_frame(&self, _flags: AllocFlags) -> Result<Frame, AllocError> {
        if !self.initialized.load(Ordering::Acquire) {
            return Err(AllocError::NotInitialized);
        }

        let mut inner = self.inner.lock();
        if inner.free_frames == 0 {
            return Err(AllocError::OutOfMemory);
        }

        // Search starting from hint (avoids systematic O(n))
        let start_word = inner.next_hint / 64;
        let nwords = BITMAP_WORDS;

        for i in 0..nwords {
            let word_idx = (start_word + i) % nwords;
            let word_val = inner.bitmap[word_idx];
            if word_val == u64::MAX {
                continue; // All allocated in this word
            }
            // Find first 0 bit (free frame)
            let bit = (!word_val).trailing_zeros() as usize;
            let pfn = word_idx * 64 + bit;
            if pfn >= inner.total_frames {
                continue;
            }
            // Mark as allocated
            inner.bitmap[word_idx] |= 1u64 << bit;
            inner.free_frames -= 1;
            inner.next_hint = pfn + 1;

            let phys = PhysAddr::new(inner.phys_start + pfn as u64 * PAGE_SIZE as u64);
            return Ok(Frame::containing(phys));
        }

        Err(AllocError::OutOfMemory)
    }

    /// Frees a previously allocated frame.
    pub fn free_frame(&self, frame: Frame) {
        if !self.initialized.load(Ordering::Acquire) {
            return;
        }

        let mut inner = self.inner.lock();
        let phys = frame.start_address();
        if phys.as_u64() < inner.phys_start || phys.as_u64() >= inner.phys_end {
            return;
        }
        let pfn = ((phys.as_u64() - inner.phys_start) / PAGE_SIZE as u64) as usize;
        if pfn >= BOOTSTRAP_MAX_FRAMES {
            return;
        }
        let word = pfn / 64;
        let bit = pfn % 64;
        debug_assert!(
            (inner.bitmap[word] >> bit) & 1 == 1,
            "free_frame: frame déjà libre (double-free)"
        );
        inner.bitmap[word] &= !(1u64 << bit);
        inner.free_frames += 1;
        if pfn < inner.next_hint {
            inner.next_hint = pfn;
        }
    }

    /// Returns the number of free frames.
    pub fn free_frames(&self) -> usize {
        self.inner.lock().free_frames
    }

    /// Checks if a frame is free.
    pub fn is_free(&self, frame: Frame) -> bool {
        self.tracked_is_free(frame).unwrap_or(false)
    }

    /// Returns frame state if this frame belongs to the window tracked
    /// by the bootstrap bitmap.
    pub fn tracked_is_free(&self, frame: Frame) -> Option<bool> {
        let inner = self.inner.lock();
        let phys = frame.start_address();
        if phys.as_u64() < inner.phys_start || phys.as_u64() >= inner.phys_end {
            return None;
        }
        let pfn = ((phys.as_u64() - inner.phys_start) / PAGE_SIZE as u64) as usize;
        if pfn >= BOOTSTRAP_MAX_FRAMES {
            return None;
        }
        let word = pfn / 64;
        let bit = pfn % 64;
        Some((inner.bitmap[word] >> bit) & 1 == 0)
    }
}

/// Global bootstrap bitmap allocator.
/// Used during boot phase before buddy allocator is operational.
pub static BOOTSTRAP_BITMAP: BitmapAllocator = BitmapAllocator::new();
