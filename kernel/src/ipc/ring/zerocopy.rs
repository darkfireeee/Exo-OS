// kernel/src/ipc/ring/zerocopy.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ZERO-COPY RING — physical page sharing between producer and consumer
// (Exo-OS · IPC Layer 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// For large data volumes (>= 4 KB), copying data into the
// ring would be too costly. Zero-copy shares the physical page directly
// between both address spaces via the NO_COW flag.
//
// FLOW:
//   1. Producer allocates a buffer from the SHM pool
//      (shared_memory/pool.rs: alloc <100 ns).
//   2. Producer writes its data inside.
//   3. Producer sends a ZeroCopyRef (24 bytes) in the normal SpscRing.
//   4. Consumer receives the ZeroCopyRef, reads data without copying.
//   5. Consumer releases the buffer back to the SHM pool.
//
// SECURITY:
//   • NO_COW mapping forbids CoW from creating transparent copies.
//   • Access rights are validated via security::access_control::check_access() prior to mapping.
//   • The page is immutable on consumer side (R/O mapping).
// ═══════════════════════════════════════════════════════════════════════════════

pub use crate::ipc::core::transfer::ZeroCopyRef;
use crate::ipc::core::{array_index_nospec, IpcError};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// ZeroCopyBuffer — SHM buffer with reference count
// ─────────────────────────────────────────────────────────────────────────────

/// Metadata of an active zero-copy buffer.
/// Stored in the SHM pool alongside data.
#[repr(C, align(8))]
pub struct ZeroCopyBuffer {
    /// Physical address of data start (aligned to PAGE_SIZE).
    pub phys_addr: u64,
    /// Total buffer size in bytes.
    pub capacity: u32,
    /// Number of bytes written (useful length).
    pub length: AtomicU32,
    /// Reference count: 1 = producer only, 2 = prod + cons, 0 = free.
    pub refcount: AtomicU32,
    /// Index in SHM pool (for release).
    pub pool_idx: u32,
    /// Slot generation (detects UAF reuses).
    pub generation: AtomicU64,
}

impl ZeroCopyBuffer {
    /// Creates an uninitialized ZeroCopyBuffer (for use in statics/tables).
    /// refcount = 0 = free buffer, never used without prior init.
    pub const fn new_uninit() -> Self {
        Self {
            phys_addr: 0,
            capacity: 0,
            length: AtomicU32::new(0),
            refcount: AtomicU32::new(0),
            pool_idx: 0,
            generation: AtomicU64::new(0),
        }
    }

    /// Initializes a buffer from an SHM allocation.
    pub fn init(phys_addr: u64, capacity: u32, pool_idx: u32) -> Self {
        Self {
            phys_addr,
            capacity,
            length: AtomicU32::new(0),
            refcount: AtomicU32::new(1), // starts at 1 (producer)
            pool_idx,
            generation: AtomicU64::new(1),
        }
    }

    /// Increments refcount (addition of a receiver).
    #[inline(always)]
    pub fn acquire(&self) {
        self.refcount.fetch_add(1, Ordering::Acquire);
    }

    /// Decrements refcount. Returns true if buffer can be released.
    #[inline(always)]
    pub fn release(&self) -> bool {
        let mut cur = self.refcount.load(Ordering::Acquire);
        loop {
            if cur == 0 {
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
                        // Last user — invalidate generation.
                        self.generation.fetch_add(1, Ordering::AcqRel);
                        return true;
                    }
                    return false;
                }
                Err(actual) => cur = actual,
            }
        }
    }

    /// Constructs a `ZeroCopyRef` to send into the ring.
    #[inline(always)]
    pub fn to_ref(&self) -> ZeroCopyRef {
        ZeroCopyRef::new(
            self.phys_addr,
            self.length.load(Ordering::Acquire),
            self.pool_idx,
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ZeroCopyRing — specialized SPSC ring for ZeroCopyRef
// ─────────────────────────────────────────────────────────────────────────────

/// SPSC ring for zero-copy reference exchange.
///
/// Reduced ring size because zero-copy transfers are rarely very
/// numerous simultaneously (mainly DMA / video streaming).
const ZC_RING_SIZE: usize = 512;
const ZC_RING_MASK: usize = ZC_RING_SIZE - 1;

/// A slot of the zero-copy ring — contains only a ZeroCopyRef.
#[repr(C, align(32))]
struct ZcSlot {
    seq: AtomicU64,
    zc: ZeroCopyRef,
    _pad: [u8; 4],
}

impl ZcSlot {
    const fn zeroed() -> Self {
        Self {
            seq: AtomicU64::new(0),
            zc: ZeroCopyRef {
                phys_addr: 0,
                length: 0,
                pool_idx: 0,
            },
            _pad: [0u8; 4],
        }
    }
}

const _: () = assert!(
    core::mem::size_of::<ZcSlot>() == 32,
    "ZcSlot doit faire 32 bytes"
);

/// SPSC ring for zero-copy exchanges.
pub struct ZeroCopyRing {
    head: AtomicU64,
    _pad1: [u8; 56],
    tail: AtomicU64,
    _pad2: [u8; 56],
    slots: [ZcSlot; ZC_RING_SIZE],
}

// SAFETY: access controlled by head/tail/sequence.
unsafe impl Send for ZeroCopyRing {}
unsafe impl Sync for ZeroCopyRing {}

impl ZeroCopyRing {
    /// Creates a zero-copy ring.
    pub const fn new() -> Self {
        Self {
            head: AtomicU64::new(0),
            _pad1: [0u8; 56],
            tail: AtomicU64::new(0),
            _pad2: [0u8; 56],
            slots: [const { ZcSlot::zeroed() }; ZC_RING_SIZE],
        }
    }

    /// Initializes sequences.
    pub fn init(&mut self) {
        for (i, slot) in self.slots.iter_mut().enumerate() {
            slot.seq.store(i as u64, Ordering::Relaxed);
        }
        core::sync::atomic::fence(Ordering::Release);
    }

    /// Spectre-safe access (RULE IPC-08).
    #[inline(always)]
    fn slot_at(&self, pos: u64) -> &ZcSlot {
        let idx = array_index_nospec((pos as usize) & ZC_RING_MASK, ZC_RING_SIZE);
        &self.slots[idx]
    }

    /// Sends a zero-copy reference.
    pub fn push(&self, zc: ZeroCopyRef) -> Result<(), IpcError> {
        let pos = self.head.load(Ordering::Relaxed);
        let slot = self.slot_at(pos);
        if slot.seq.load(Ordering::Acquire) != pos {
            return Err(IpcError::WouldBlock);
        }
        // SAFETY: SPSC — single producer. addr_of! avoids creating an intermediate &T,
        // which avoids strict-aliasing UB.
        // SAFETY: addr_of! avoids strict-aliasing UB; array_index_nospec (IPC-08).
        unsafe {
            let safe_idx = array_index_nospec((pos as usize) & ZC_RING_MASK, ZC_RING_SIZE);
            let s = core::ptr::addr_of!(self.slots[safe_idx]);
            core::ptr::write(core::ptr::addr_of!((*s).zc) as *mut ZeroCopyRef, zc);
        }
        slot.seq.store(pos + 1, Ordering::Release);
        self.head.store(pos + 1, Ordering::Relaxed);
        Ok(())
    }

    /// Receives a zero-copy reference.
    pub fn pop(&self) -> Result<ZeroCopyRef, IpcError> {
        let pos = self.tail.load(Ordering::Relaxed);
        let slot = self.slot_at(pos);
        if slot.seq.load(Ordering::Acquire) != pos + 1 {
            return Err(IpcError::WouldBlock);
        }
        // SAFETY: validated sequence -> data available. SPSC -> single consumer.
        let zc = unsafe {
            let safe_idx = array_index_nospec((pos as usize) & ZC_RING_MASK, ZC_RING_SIZE);
            core::ptr::read(&self.slots[safe_idx].zc) // IPC-08: safe_idx bounded by nospec
        };
        slot.seq.store(pos + ZC_RING_SIZE as u64, Ordering::Release);
        self.tail.store(pos + 1, Ordering::Relaxed);
        Ok(zc)
    }

    /// Returns true if ring is empty.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        let pos = self.tail.load(Ordering::Relaxed);
        self.slot_at(pos).seq.load(Ordering::Relaxed) != pos + 1
    }
}
