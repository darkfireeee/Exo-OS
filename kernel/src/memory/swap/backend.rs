// kernel/src/memory/swap/backend.rs
//
// Swap backend — manages swap devices (partition or file).
// Layer 0 — no dependencies towards fs/process/scheduler.
// Integration with the FS is done via the `SwapDevice` trait.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::memory::core::types::PhysAddr;

// ─────────────────────────────────────────────────────────────────────────────
// SWAP DEVICE ABSTRACTION TRAIT
// ─────────────────────────────────────────────────────────────────────────────

/// Swap slot identifier (index of a page in the swap).
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Debug)]
#[repr(transparent)]
pub struct SwapSlot(pub u64);

impl SwapSlot {
    pub const INVALID: Self = SwapSlot(u64::MAX);
    #[inline]
    pub fn is_valid(self) -> bool {
        self.0 != u64::MAX
    }
    #[inline]
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

/// Swap device abstraction trait.
/// Implemented by the FS/block layer subsystem, but defined here (Layer 0).
pub trait SwapDevice: Send + Sync {
    /// Reads a swap page from the device into physical memory `dst`.
    ///
    /// # Safety
    /// `dst` must be a valid physical frame.
    unsafe fn read_page(&self, slot: SwapSlot, dst: PhysAddr) -> Result<(), SwapError>;

    /// Writes a swap page from physical memory `src` to the device.
    ///
    /// # Safety
    /// `src` must be a valid physical frame.
    unsafe fn write_page(&self, slot: SwapSlot, src: PhysAddr) -> Result<(), SwapError>;

    /// Allocates a free slot in the device.
    fn alloc_slot(&self) -> Option<SwapSlot>;

    /// Frees a slot (the page has been permanently reloaded into RAM).
    fn free_slot(&self, slot: SwapSlot);

    /// Returns total capacity in pages.
    fn capacity_pages(&self) -> u64;

    /// Returns the number of free pages.
    fn free_pages(&self) -> u64;
}

// ─────────────────────────────────────────────────────────────────────────────
// SWAP ERRORS
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(u8)]
pub enum SwapError {
    NoDevice = 0,
    NoSlot = 1,
    IoError = 2,
    InvalidSlot = 3,
    DeviceFull = 4,
    NotEnabled = 5,
    Corrupted = 6,
}

// ─────────────────────────────────────────────────────────────────────────────
// SWAP DEVICE REGISTRY
// ─────────────────────────────────────────────────────────────────────────────

pub const MAX_SWAP_DEVICES: usize = 8;

/// Table of registered swap devices.
pub struct SwapBackendRegistry {
    devices: [Option<&'static dyn SwapDevice>; MAX_SWAP_DEVICES],
    count: AtomicU32,
    enabled: AtomicBool,
    // Global statistics.
    pub stats_reads: AtomicU64,
    pub stats_writes: AtomicU64,
    pub stats_errors: AtomicU64,
    pub stats_slots_freed: AtomicU64,
}

// SAFETY: devices is protected by AtomicU32 (write-once per slot during init).
unsafe impl Sync for SwapBackendRegistry {}
unsafe impl Send for SwapBackendRegistry {}

impl SwapBackendRegistry {
    const fn new() -> Self {
        SwapBackendRegistry {
            devices: [None; MAX_SWAP_DEVICES],
            count: AtomicU32::new(0),
            enabled: AtomicBool::new(false),
            stats_reads: AtomicU64::new(0),
            stats_writes: AtomicU64::new(0),
            stats_errors: AtomicU64::new(0),
            stats_slots_freed: AtomicU64::new(0),
        }
    }

    /// Registers a swap device.
    ///
    /// # Safety
    /// `device` must have a static lifetime.
    pub unsafe fn register(&self, device: &'static dyn SwapDevice) -> Result<(), SwapError> {
        let idx = self.count.load(Ordering::Relaxed) as usize;
        if idx >= MAX_SWAP_DEVICES {
            return Err(SwapError::DeviceFull);
        }
        // SAFETY: addr_of! avoids &T->*mut T; write-once per slot guaranteed by atomic count.
        let ptr = core::ptr::addr_of!(self.devices[idx]) as *mut Option<&'static dyn SwapDevice>;
        core::ptr::write(ptr, Some(device));
        self.count.fetch_add(1, Ordering::Release);
        self.enabled.store(true, Ordering::Release);
        Ok(())
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    /// Allocates a slot on the first device with space available.
    pub fn alloc_slot(&self) -> Result<(usize, SwapSlot), SwapError> {
        let count = self.count.load(Ordering::Acquire) as usize;
        for i in 0..count {
            if let Some(dev) = self.devices[i] {
                if let Some(slot) = dev.alloc_slot() {
                    return Ok((i, slot));
                }
            }
        }
        Err(SwapError::NoSlot)
    }

    /// Frees a slot on device `dev_idx`.
    pub fn free_slot(&self, dev_idx: usize, slot: SwapSlot) {
        if dev_idx < MAX_SWAP_DEVICES {
            if let Some(dev) = self.devices[dev_idx] {
                dev.free_slot(slot);
                self.stats_slots_freed.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Reads a page from swap.
    ///
    /// # Safety
    /// `dst` must be a valid physical frame.
    pub unsafe fn read_page(
        &self,
        dev_idx: usize,
        slot: SwapSlot,
        dst: PhysAddr,
    ) -> Result<(), SwapError> {
        if dev_idx >= MAX_SWAP_DEVICES {
            return Err(SwapError::InvalidSlot);
        }
        let dev = self.devices[dev_idx].ok_or(SwapError::NoDevice)?;
        let res = dev.read_page(slot, dst);
        match &res {
            Ok(_) => {
                self.stats_reads.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                self.stats_errors.fetch_add(1, Ordering::Relaxed);
            }
        }
        res
    }

    /// Writes a page to swap.
    ///
    /// # Safety
    /// `src` must be a valid physical frame.
    pub unsafe fn write_page(
        &self,
        dev_idx: usize,
        slot: SwapSlot,
        src: PhysAddr,
    ) -> Result<(), SwapError> {
        if dev_idx >= MAX_SWAP_DEVICES {
            return Err(SwapError::InvalidSlot);
        }
        let dev = self.devices[dev_idx].ok_or(SwapError::NoDevice)?;
        let res = dev.write_page(slot, src);
        match &res {
            Ok(_) => {
                self.stats_writes.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                self.stats_errors.fetch_add(1, Ordering::Relaxed);
            }
        }
        res
    }

    /// Total capacity in pages (sum of all devices).
    pub fn total_capacity(&self) -> u64 {
        let count = self.count.load(Ordering::Acquire) as usize;
        (0..count)
            .filter_map(|i| self.devices[i])
            .map(|d| d.capacity_pages())
            .sum()
    }

    /// Total free pages.
    pub fn total_free(&self) -> u64 {
        let count = self.count.load(Ordering::Acquire) as usize;
        (0..count)
            .filter_map(|i| self.devices[i])
            .map(|d| d.free_pages())
            .sum()
    }
}

pub static SWAP_BACKEND: SwapBackendRegistry = SwapBackendRegistry::new();

// ─────────────────────────────────────────────────────────────────────────────
// SWAP ENTRY (in the page table)
// ─────────────────────────────────────────────────────────────────────────────

/// Encoding of a swap entry in the PTE (not present with swap info).
/// Format: bits [63:12] = slot, bits [11:9] = dev_idx, bit [0] = 0 (not present).
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(transparent)]
pub struct SwapPte(pub u64);

impl SwapPte {
    pub const SWAP_FLAG: u64 = 0b10; // bit 1 = swap marker (bit 0 = 0 = not present)

    pub fn encode(dev_idx: usize, slot: SwapSlot) -> Self {
        // FIX-SWAP-SWAP-PTE-01: dev_idx must be in bits [11:9] as documented.
        // The old shift (<< 1) overwrote SWAP_FLAG (bit 1) and caused
        // dev_idx(encode(0, s)) == 1 (false round-trip).
        let s = (slot.as_u64() << 12) | ((dev_idx as u64 & 0x7) << 9) | Self::SWAP_FLAG;
        SwapPte(s)
    }

    pub fn is_swap(raw: u64) -> bool {
        raw & 1 == 0 && raw & Self::SWAP_FLAG != 0
    }

    pub fn dev_idx(self) -> usize {
        ((self.0 >> 9) & 0x7) as usize
    }
    pub fn slot(self) -> SwapSlot {
        SwapSlot(self.0 >> 12)
    }
    pub fn raw(self) -> u64 {
        self.0
    }
}
