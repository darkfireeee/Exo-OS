// kernel/src/memory/physical/frame/descriptor.rs
//
// FrameDesc — descriptor of a physical frame.
// Each physical frame in the system has exactly one associated FrameDesc
// in the global `FRAME_TABLE` array.
// Layer 0 — no external dependencies.

use crate::memory::core::types::ZoneType;
use core::sync::atomic::{AtomicU16, AtomicU32, AtomicU8, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// FRAME FLAGS — frame state and properties
// ─────────────────────────────────────────────────────────────────────────────

/// State flags of a physical frame.
/// Represented on 16 bits to fit inside FrameDesc.
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct FrameFlags(u16);

impl FrameFlags {
    /// Free frame (not allocated).
    pub const FREE: FrameFlags = FrameFlags(1 << 0);
    /// Frame allocated by kernel.
    pub const KERNEL: FrameFlags = FrameFlags(1 << 1);
    /// Frame allocated for userspace.
    pub const USER: FrameFlags = FrameFlags(1 << 2);
    /// Frame used for DMA (no-cache, no-swap, no-CoW).
    pub const DMA: FrameFlags = FrameFlags(1 << 3);
    /// Frame locked in RAM (do not swap/migrate).
    pub const PINNED: FrameFlags = FrameFlags(1 << 4);
    /// Copy-on-Write frame (shared between processes, copy on write).
    pub const COW: FrameFlags = FrameFlags(1 << 5);
    /// Dirty frame (modified since last disk synchronization).
    pub const DIRTY: FrameFlags = FrameFlags(1 << 6);
    /// Recently accessed frame (LRU clock bit).
    pub const ACCESSED: FrameFlags = FrameFlags(1 << 7);
    /// Frame belonging to a huge page (2 MiB).
    pub const HUGE_PAGE: FrameFlags = FrameFlags(1 << 8);
    /// Frame belonging to a swap page.
    pub const SWAP: FrameFlags = FrameFlags(1 << 9);
    /// Frame in a per-CPU pool (hot cache).
    pub const IN_CPU_POOL: FrameFlags = FrameFlags(1 << 10);
    /// Frame shared across multiple processes (SHM/mmap).
    pub const SHARED: FrameFlags = FrameFlags(1 << 11);
    /// Frame currently migrating across NUMA nodes.
    pub const MIGRATING: FrameFlags = FrameFlags(1 << 12);
    /// Frame reserved by firmware/BIOS (never allocated).
    pub const RESERVED: FrameFlags = FrameFlags(1 << 13);
    /// Frame in active LRU list (LRU policy).
    pub const LRU_ACTIVE: FrameFlags = FrameFlags(1 << 14);
    /// Frame in inactive LRU list.
    pub const LRU_INACTIVE: FrameFlags = FrameFlags(1 << 15);

    /// Locked DMA frame (combination DMA + PINNED — V-05 / MEM-05).
    /// Must be set on any frame allocated for DMA and cleared after
    /// `wait_dma_complete()`. Protects against reclaim and swap.
    pub const DMA_PINNED: FrameFlags = FrameFlags(Self::DMA.0 | Self::PINNED.0);

    pub const EMPTY: FrameFlags = FrameFlags(0);

    #[inline(always)]
    pub const fn contains(self, f: FrameFlags) -> bool {
        (self.0 & f.0) == f.0
    }

    #[inline(always)]
    pub const fn set(self, f: FrameFlags) -> Self {
        FrameFlags(self.0 | f.0)
    }

    #[inline(always)]
    pub const fn clear(self, f: FrameFlags) -> Self {
        FrameFlags(self.0 & !f.0)
    }

    #[inline(always)]
    pub const fn bits(self) -> u16 {
        self.0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// FRAME DESCRIPTOR
// ─────────────────────────────────────────────────────────────────────────────

/// Descriptor of a physical frame.
///
/// Must fit in exactly 32 bytes so that the global table
/// `FRAME_TABLE[nr_frames]` remains cache-friendly (2 descriptors per
/// 64-byte cache line).
///
/// Layout (32 bytes):
///   - refcount (4 bytes, AtomicU32) : reference counter (CoW/SHM)
///   - flags    (2 bytes, AtomicU16) : frame state and properties
///   - zone     (1 byte,  u8)        : ZoneType (Dma/Dma32/Normal/High/Movable)
///   - numa_node(1 byte,  u8)        : owning NUMA node (0..MAX_NUMA_NODES)
///   - order    (1 byte,  AtomicU8)  : buddy order (0=4KiB, 9=2MiB, ...)
///     if > BUDDY_MAX_ORDER -> in DMA pool or reserved
///   - lru_gen  (1 byte,  AtomicU8)  : LRU generation (for clock algorithm)
///   - mapcount (2 bytes, AtomicU16) : number of page tables mapping this frame
///   - _pad     (20 bytes)           : future alignment / 32 bytes total
#[repr(C, align(32))]
pub struct FrameDesc {
    /// Atomic reference counter — 0 = free frame.
    pub refcount: AtomicU32,
    /// Atomic state flags.
    pub flags: AtomicU16,
    /// Frame memory zone.
    pub zone: u8,
    /// Owning NUMA node.
    pub numa_node: u8,
    /// Current buddy order (0..=12). BUDDY_MAX_ORDER+1 = outside buddy.
    pub order: AtomicU8,
    /// LRU generation (clock bit, incremented at each scan).
    pub lru_gen: AtomicU8,
    /// Active mappings count for this frame (for writable/COW accounting).
    pub mapcount: AtomicU16,
    /// Reserved for future use (padding to 32 bytes).
    _pad: [u8; 20],
}

const _: () = assert!(
    core::mem::size_of::<FrameDesc>() == 32,
    "FrameDesc doit faire exactement 32 bytes"
);
const _: () = assert!(
    core::mem::align_of::<FrameDesc>() == 32,
    "FrameDesc doit être aligné sur 32 bytes"
);

impl FrameDesc {
    /// Creates a free FrameDesc in the specified zone and NUMA node.
    #[inline]
    pub const fn new_free(zone: ZoneType, numa_node: u8) -> Self {
        FrameDesc {
            refcount: AtomicU32::new(0),
            flags: AtomicU16::new(FrameFlags::FREE.0),
            zone: zone as u8,
            numa_node,
            order: AtomicU8::new(0),
            lru_gen: AtomicU8::new(0),
            mapcount: AtomicU16::new(0),
            _pad: [0u8; 20],
        }
    }

    /// Returns the frame's memory zone.
    #[inline(always)]
    pub fn zone_type(&self) -> ZoneType {
        ZoneType::from_index(self.zone as usize).unwrap_or(ZoneType::Normal)
    }

    /// Returns the frame's NUMA node.
    #[inline(always)]
    pub fn numa_node(&self) -> u8 {
        self.numa_node
    }

    // ── Refcount ────────────────────────────────────────────────────────────

    /// Returns the current refcount.
    #[inline(always)]
    pub fn refcount(&self) -> u32 {
        self.refcount.load(Ordering::Relaxed)
    }

    /// Atomically increments the refcount.
    /// Returns the new refcount.
    #[inline(always)]
    pub fn inc_ref(&self) -> u32 {
        self.refcount.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Atomically decrements the refcount.
    /// Returns `true` if the frame should be freed (refcount reached 0).
    ///
    /// # Safety – double-free guard
    /// If the refcount is already 0, decrementing would be a double-free.
    /// We cancel the operation, restore 0, and return `false`.
    /// In debug mode, panics immediately.
    #[inline(always)]
    pub fn dec_ref(&self) -> bool {
        let prev = self.refcount.fetch_sub(1, Ordering::AcqRel);
        if prev == 0 {
            // Double-free: reset counter to 0 to prevent u32::MAX wrapping
            self.refcount.store(0, Ordering::Release);
            // In debug: panic to ease detection
            debug_assert!(
                false,
                "dec_ref avec refcount déjà à 0 — double-free détecté"
            );
            return false;
        }
        prev == 1
    }

    /// Sets the refcount to an arbitrary value (boot only).
    ///
    /// # Safety
    /// Must only be called during frame table initialization,
    /// before other CPUs have access to the descriptor.
    #[inline(always)]
    pub unsafe fn set_refcount_init(&self, count: u32) {
        self.refcount.store(count, Ordering::Relaxed);
    }

    // ── Flags ────────────────────────────────────────────────────────────────

    /// Returns current flags.
    #[inline(always)]
    pub fn flags(&self) -> FrameFlags {
        FrameFlags(self.flags.load(Ordering::Relaxed))
    }

    /// Checks whether a flag is present.
    #[inline(always)]
    pub fn has_flag(&self, flag: FrameFlags) -> bool {
        self.flags().contains(flag)
    }

    /// Sets a flag atomically.
    #[inline(always)]
    pub fn set_flag(&self, flag: FrameFlags) {
        self.flags.fetch_or(flag.bits(), Ordering::Relaxed);
    }

    /// Clears a flag atomically.
    #[inline(always)]
    pub fn clear_flag(&self, flag: FrameFlags) {
        self.flags.fetch_and(!flag.bits(), Ordering::Relaxed);
    }

    /// Atomically swaps flags (returns old ones).
    #[inline(always)]
    pub fn swap_flags(&self, new_flags: FrameFlags) -> FrameFlags {
        FrameFlags(self.flags.swap(new_flags.bits(), Ordering::AcqRel))
    }

    /// Checks whether the frame is free.
    #[inline(always)]
    pub fn is_free(&self) -> bool {
        self.has_flag(FrameFlags::FREE)
    }

    /// Checks whether the frame is pinned (non-swappable).
    #[inline(always)]
    pub fn is_pinned(&self) -> bool {
        self.has_flag(FrameFlags::PINNED)
    }

    /// Checks whether the frame is CoW.
    #[inline(always)]
    pub fn is_cow(&self) -> bool {
        self.has_flag(FrameFlags::COW)
    }

    /// Checks whether the frame is reserved (firmware).
    #[inline(always)]
    pub fn is_reserved(&self) -> bool {
        self.has_flag(FrameFlags::RESERVED)
    }

    // ── Order (buddy) ────────────────────────────────────────────────────────

    /// Returns current buddy order.
    #[inline(always)]
    pub fn order(&self) -> u8 {
        self.order.load(Ordering::Relaxed)
    }

    /// Sets buddy order.
    #[inline(always)]
    pub fn set_order(&self, order: u8) {
        self.order.store(order, Ordering::Relaxed);
    }

    // ── Mapcount ─────────────────────────────────────────────────────────────

    /// Returns the number of active mappings.
    #[inline(always)]
    pub fn mapcount(&self) -> u16 {
        self.mapcount.load(Ordering::Relaxed)
    }

    /// Increments mapcount.
    #[inline(always)]
    pub fn inc_mapcount(&self) -> u16 {
        self.mapcount.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Decrements mapcount. Returns `true` if mapcount reaches 0.
    ///
    /// # Safety – underflow guard
    /// If mapcount is already 0, cancels decrement (resets to 0) and returns `false`.
    #[inline(always)]
    pub fn dec_mapcount(&self) -> bool {
        let prev = self.mapcount.fetch_sub(1, Ordering::AcqRel);
        if prev == 0 {
            // Underflow: reset to 0 to prevent wrapping to u16::MAX
            self.mapcount.store(0, Ordering::Relaxed);
            debug_assert!(false, "mapcount underflow");
            return false;
        }
        prev == 1
    }

    // ── LRU ─────────────────────────────────────────────────────────────────

    /// Returns current LRU generation.
    #[inline(always)]
    pub fn lru_gen(&self) -> u8 {
        self.lru_gen.load(Ordering::Relaxed)
    }

    /// Increments LRU generation (saturating at u8::MAX).
    #[inline(always)]
    pub fn touch_lru(&self) {
        let cur = self.lru_gen.load(Ordering::Relaxed);
        if cur < u8::MAX {
            self.lru_gen.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Resets LRU generation to 0 (old frame -> eviction candidate).
    #[inline(always)]
    pub fn reset_lru(&self) {
        self.lru_gen.store(0, Ordering::Relaxed);
    }

    // ── Transition free <-> allocated ────────────────────────────────────────

    /// Marks the frame as allocated (clears FREE, increments refcount to 1).
    /// Returns false if the frame was not free (in debug: panics).
    #[inline]
    pub fn mark_allocated(&self, kernel: bool) -> bool {
        let prev_flags = FrameFlags(
            self.flags
                .fetch_and(!FrameFlags::FREE.bits(), Ordering::AcqRel),
        );
        if !prev_flags.contains(FrameFlags::FREE) {
            debug_assert!(false, "mark_allocated: frame n'était pas FREE");
            return false;
        }
        let access_flag = if kernel {
            FrameFlags::KERNEL
        } else {
            FrameFlags::USER
        };
        self.set_flag(access_flag);
        self.refcount.store(1, Ordering::Release);
        true
    }

    /// Marks the frame as free (resets flags to FREE, refcount to 0).
    ///
    /// # Safety
    /// Caller must guarantee no further reference exists.
    /// If refcount != 0 in release, refuses to mark frame free to avoid
    /// use-after-free (returns without modifying flags).
    #[inline]
    pub unsafe fn mark_free(&self) {
        let rc = self.refcount.load(Ordering::Acquire);
        if rc != 0 {
            // Potential use-after-free: non-zero refcount when attempting
            // to free frame. We refuse the operation.
            debug_assert_eq!(rc, 0, "mark_free avec refcount != 0");
            return;
        }
        self.mapcount.store(0, Ordering::Relaxed);
        self.order.store(0, Ordering::Relaxed);
        self.lru_gen.store(0, Ordering::Relaxed);
        self.flags.store(FrameFlags::FREE.bits(), Ordering::Release);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GLOBAL FRAME DESCRIPTOR TABLE (FRAME_DESCRIPTORS)
// ─────────────────────────────────────────────────────────────────────────────
//
// Each physical frame has exactly one FrameDescEntry in this table.
// Indexed by PFN (Physical Frame Number).
//
// Size: MAX_PHYS_FRAMES x 2 bytes = 4 MiB (for 8 GiB max RAM).
// Stored in BSS — zero-initialized at boot.
//
// Typical accesses:
//   FRAME_DESCRIPTORS.get(frame).flags()           -> read flags
//   FRAME_DESCRIPTORS.get(frame).set_flag(f)       -> set flag
//   FRAME_DESCRIPTORS.get(frame).clear_flag(f)     -> clear flag
//   FRAME_DESCRIPTORS.get(frame).test_and_clear(f) -> atomic CAS
//
// RULE: Always check that pfn < MAX_PHYS_FRAMES before access.
//       Frames beyond 8 GiB physical are not covered.
//       These frames must not be allocated (zone guard).

use crate::memory::core::types::Frame;

/// Maximum number of frames covered by the global table.
/// 2^21 frames x 4 KiB/frame = 8 GiB RAM max.
pub const MAX_PHYS_FRAMES: usize = 1 << 21;

/// Compact entry of the global table — 2 bytes per frame.
/// Contains only LRU/CoW/DIRTY/PINNED/DMA flags.
#[repr(transparent)]
pub struct FrameDescEntry {
    flags: AtomicU16,
}

impl FrameDescEntry {
    const fn new() -> Self {
        FrameDescEntry {
            flags: AtomicU16::new(0),
        }
    }

    /// Returns current flags.
    #[inline(always)]
    pub fn flags(&self) -> FrameFlags {
        FrameFlags(self.flags.load(Ordering::Relaxed))
    }

    /// Sets a flag.
    #[inline(always)]
    pub fn set_flag(&self, f: FrameFlags) {
        self.flags.fetch_or(f.bits(), Ordering::Relaxed);
    }

    /// Clears a flag.
    #[inline(always)]
    pub fn clear_flag(&self, f: FrameFlags) {
        self.flags.fetch_and(!f.bits(), Ordering::Relaxed);
    }

    /// Atomic test-and-clear.
    /// Returns `true` if flag was set before clearing.
    #[inline]
    pub fn test_and_clear(&self, f: FrameFlags) -> bool {
        let prev = self.flags.fetch_and(!f.bits(), Ordering::AcqRel);
        FrameFlags(prev).contains(f)
    }

    /// Replaces flags entirely (returns old ones).
    #[inline(always)]
    pub fn swap_flags(&self, new: FrameFlags) -> FrameFlags {
        FrameFlags(self.flags.swap(new.bits(), Ordering::AcqRel))
    }
}

/// Global table of all lightweight physical frame descriptors.
pub struct FrameDescriptorTable {
    entries: [FrameDescEntry; MAX_PHYS_FRAMES],
}

// SAFETY: FrameDescEntry only contains an AtomicU16,
// which is Send + Sync by nature. The table is therefore Send + Sync.
unsafe impl Sync for FrameDescriptorTable {}
unsafe impl Send for FrameDescriptorTable {}

impl FrameDescriptorTable {
    /// Returns a reference to the entry for the given frame.
    ///
    /// # Panics (debug)
    /// If `frame.pfn() >= MAX_PHYS_FRAMES`, panics in debug mode,
    /// saturates to `MAX_PHYS_FRAMES - 1` in release mode.
    #[inline]
    pub fn get(&self, frame: Frame) -> &FrameDescEntry {
        let pfn = frame.pfn() as usize;
        debug_assert!(
            pfn < MAX_PHYS_FRAMES,
            "FrameDescriptorTable::get: PFN {} hors limites (max {})",
            pfn,
            MAX_PHYS_FRAMES
        );
        // SAFETY: table[pfn] is valid for pfn < MAX_PHYS_FRAMES.
        &self.entries[pfn.min(MAX_PHYS_FRAMES - 1)]
    }

    /// Returns a direct reference (unsafe, without bounds checking).
    ///
    /// # Safety
    /// Caller must guarantee `pfn < MAX_PHYS_FRAMES`.
    #[inline(always)]
    pub unsafe fn get_unchecked(&self, pfn: u64) -> &FrameDescEntry {
        &*self.entries.as_ptr().add(pfn as usize)
    }
}

/// Global frame descriptor table.
/// In BSS — zero-initialized (all flags at 0 = EMPTY).
/// Upon frame allocation, `mark_allocated()` sets the appropriate flags.
pub static FRAME_DESCRIPTORS: FrameDescriptorTable = FrameDescriptorTable {
    entries: [const { FrameDescEntry::new() }; MAX_PHYS_FRAMES],
};
