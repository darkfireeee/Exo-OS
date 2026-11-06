// ipc/shared_memory/page.rs — Shared pages management (NO_COW) for Exo-OS
//
// This module manages physical pages used for IPC shared memory.
// Each page is identified by its physical address and holds an atomic reference
// count. The NO_COW flag is mandatory: the page must never be duplicated
// upon fork (Exo-OS architectural constraint).
//
// Rules:
//   RULE NO-ALLOC: no Vec/Box on hot path
//   FLAG NO_COW mandatory on all IPC SHM pages

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Shared page flags
// ---------------------------------------------------------------------------

/// Mapping flags of an SHM page
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageFlags(pub u32);

impl PageFlags {
    /// Readable page
    pub const READ: Self = Self(1 << 0);
    /// Writable page
    pub const WRITE: Self = Self(1 << 1);
    /// Executable page (normally forbidden for IPC SHM)
    pub const EXECUTE: Self = Self(1 << 2);
    /// No Copy-on-Write — MANDATORY for IPC SHM
    pub const NO_COW: Self = Self(1 << 3);
    /// Page pinned in memory (non-swappable)
    pub const PINNED: Self = Self(1 << 4);
    /// Page mapped in multiple address spaces
    pub const SHARED: Self = Self(1 << 5);
    /// Combined read + write + no_cow + pinned (SHM default)
    pub const SHM_DEFAULT: Self =
        Self(Self::READ.0 | Self::WRITE.0 | Self::NO_COW.0 | Self::PINNED.0 | Self::SHARED.0);

    pub fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    pub fn remove(&mut self, other: Self) {
        self.0 &= !other.0;
    }
}

// ---------------------------------------------------------------------------
// Page identification
// ---------------------------------------------------------------------------

/// Standard page size (4 KiB)
pub const PAGE_SIZE: usize = 4096;

/// Huge page size (2 MiB)
pub const HUGE_PAGE_SIZE: usize = 2 * 1024 * 1024;

/// Physical address aligned to PAGE_SIZE — newtyped wrapper
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct PhysAddr(pub u64);

impl PhysAddr {
    /// Null physical address (sentinel)
    pub const NULL: Self = Self(0);

    #[inline]
    pub fn is_null(self) -> bool {
        self.0 == 0
    }

    /// Checks that address is aligned to `align` (must be a power of 2)
    #[inline]
    pub fn is_aligned(self, align: usize) -> bool {
        (self.0 & (align as u64 - 1)) == 0
    }

    /// Computes page index from base physical address.
    #[inline]
    pub fn page_index(self, base: PhysAddr) -> Option<usize> {
        if self.0 < base.0 {
            return None;
        }
        let offset = (self.0 - base.0) as usize;
        if offset % PAGE_SIZE != 0 {
            return None;
        }
        Some(offset / PAGE_SIZE)
    }
}

// ---------------------------------------------------------------------------
// SHM page descriptor
// ---------------------------------------------------------------------------

/// Descriptor of a physical page belonging to the IPC SHM pool.
/// Size: exactly 64 bytes (one cache line).
#[repr(C, align(64))]
pub struct ShmPage {
    /// Physical address of the page
    pub phys_addr: PhysAddr,
    /// Atomic reference count
    pub refcount: AtomicU32,
    /// Page flags (encoded PageFlags)
    pub flags: AtomicU32,
    /// Index in parent pool
    pub pool_index: AtomicU32,
    /// Generation (reuse version — ABA detection)
    pub generation: AtomicU32,
    /// Identifier of last process mapping this page
    pub last_mapper_pid: AtomicU32,
    /// Allocation timestamp (ns since boot)
    pub alloc_ts: AtomicU64,
    /// Number of times this page has been reused
    pub reuse_count: AtomicU32,
    _pad: [u8; 4],
}

// SAFETY: all mutable fields are atomic
unsafe impl Sync for ShmPage {}
unsafe impl Send for ShmPage {}

impl ShmPage {
    /// Creates an uninitialized page descriptor (phys_addr = NULL)
    pub const fn new_uninit() -> Self {
        Self {
            phys_addr: PhysAddr::NULL,
            refcount: AtomicU32::new(0),
            flags: AtomicU32::new(0),
            pool_index: AtomicU32::new(0),
            generation: AtomicU32::new(0),
            last_mapper_pid: AtomicU32::new(0),
            alloc_ts: AtomicU64::new(0),
            reuse_count: AtomicU32::new(0),
            _pad: [0u8; 4],
        }
    }

    /// Initializes page with a physical address and its flags.
    ///
    /// # Safety
    /// Must be called only once at boot, prior to any concurrent access.
    /// `phys_addr` is a non-atomic field: assignment occurs via
    /// a raw pointer write (write-once pattern on static).
    pub fn init(&self, phys: PhysAddr, pool_idx: u32, flags: PageFlags) {
        // SAFETY: init() called only once prior to any concurrent access; phys write-once on static.
        unsafe {
            self.set_phys_unchecked(phys);
        }
        self.flags
            .store(flags.0 | PageFlags::NO_COW.0, Ordering::Relaxed);
        self.pool_index.store(pool_idx, Ordering::Relaxed);
        self.refcount.store(0, Ordering::Release);
    }

    /// Increments reference count.
    /// Returns new count.
    pub fn ref_acquire(&self) -> u32 {
        self.refcount.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Decrements reference count.
    /// Returns `true` if page is now free (refcount == 0).
    pub fn ref_release(&self) -> bool {
        let mut cur = self.refcount.load(Ordering::Acquire);
        loop {
            if cur == 0 {
                // Double free prevented: avoid u32 wrap (T8-026)
                return false;
            }
            match self.refcount.compare_exchange_weak(
                cur,
                cur - 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    if cur == 1 {
                        // Increment generation to invalidate dangling references
                        self.generation.fetch_add(1, Ordering::Release);
                        self.reuse_count.fetch_add(1, Ordering::Relaxed);
                        return true;
                    }
                    return false;
                }
                Err(actual) => cur = actual,
            }
        }
    }

    pub fn refcount(&self) -> u32 {
        self.refcount.load(Ordering::Acquire)
    }

    pub fn is_free(&self) -> bool {
        self.refcount() == 0
    }

    pub fn flags(&self) -> PageFlags {
        PageFlags(self.flags.load(Ordering::Relaxed))
    }

    pub fn generation(&self) -> u32 {
        self.generation.load(Ordering::Acquire)
    }

    /// Verifies that NO_COW flag is set.
    pub fn assert_no_cow(&self) -> bool {
        self.flags().contains(PageFlags::NO_COW)
    }
}

// SAFETY: phys_addr is assigned only once at init; subsequent read is safe.
// (Rust cannot express this in the type system — UnsafeCell is normally used but
// field is actually write-once, so direct read is acceptable here.)
impl ShmPage {
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn set_phys_unchecked(&self, phys: PhysAddr) {
        // SAFETY: addr_of! creates a *const without intermediate reference — avoids UB
        // of mutation via &T. Single write at init, the field is write-once.
        core::ptr::write(core::ptr::addr_of!(self.phys_addr) as *mut PhysAddr, phys);
    }
}

// ---------------------------------------------------------------------------
// Free page iterator (for pool)
// ---------------------------------------------------------------------------

/// SHM page statistics
#[derive(Debug, Clone, Copy)]
pub struct ShmPageStats {
    pub total_pages: usize,
    pub free_pages: usize,
    pub used_pages: usize,
    pub total_reuses: u64,
}
