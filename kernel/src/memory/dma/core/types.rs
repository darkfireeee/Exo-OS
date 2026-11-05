// kernel/src/memory/dma/core/types.rs
//
// Fundamental DMA types — shared by all DMA sub-modules.
// LAYER 0 — zero external dependencies.

use crate::memory::core::constants::PAGE_SIZE;
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// IDENTIFIERS
// ─────────────────────────────────────────────────────────────────────────────

/// Unique identifier of a DMA channel (global index in channel table).
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Debug)]
#[repr(transparent)]
pub struct DmaChannelId(pub u32);

/// Identifier of an IOMMU domain.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Debug)]
#[repr(transparent)]
pub struct IommuDomainId(pub u32);

/// Identifier of an ongoing DMA transaction.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Debug)]
#[repr(transparent)]
pub struct DmaTransactionId(pub u64);

impl DmaTransactionId {
    pub const INVALID: Self = DmaTransactionId(0);

    /// Generates a unique ID via global atomic counter.
    pub fn generate() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        DmaTransactionId(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DMA ADDRESS (IOVA)
// ─────────────────────────────────────────────────────────────────────────────

/// I/O virtual address (IOVA) as seen by device after IOMMU translation.
/// Distinct from `PhysAddr` (CPU view).
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Debug)]
#[repr(transparent)]
pub struct IovaAddr(pub u64);

impl IovaAddr {
    #[inline]
    pub const fn new(v: u64) -> Self {
        IovaAddr(v)
    }
    #[inline]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
    #[inline]
    pub const fn is_aligned(self, align: u64) -> bool {
        self.0 & (align - 1) == 0
    }
    #[inline]
    pub fn page_aligned(self) -> Self {
        IovaAddr(self.0 & !(PAGE_SIZE as u64 - 1))
    }
    #[inline]
    pub const fn zero() -> Self {
        IovaAddr(0)
    }
    #[inline]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TRANSFER DIRECTION
// ─────────────────────────────────────────────────────────────────────────────

/// Direction of a DMA transfer.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(u8)]
pub enum DmaDirection {
    /// Device -> RAM (CPU read, device write).
    ToDevice = 0,
    /// RAM -> Device (device read, CPU write).
    FromDevice = 1,
    /// Bidirectional.
    Bidirection = 2,
    /// No movement (memset, test).
    None = 3,
}

// ─────────────────────────────────────────────────────────────────────────────
// DMA MAPPING FLAGS
// ─────────────────────────────────────────────────────────────────────────────

/// Flags modifying DMA mapping behavior.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(transparent)]
pub struct DmaMapFlags(pub u32);

impl DmaMapFlags {
    pub const NONE: Self = DmaMapFlags(0);
    /// Mapping must be within first 16 MiB (legacy ISA DMA).
    pub const DMA16: Self = DmaMapFlags(1 << 0);
    /// Mapping must stay below 4 GiB (DMA32).
    pub const DMA32: Self = DmaMapFlags(1 << 1);
    /// Buffer is physically contiguous (no scatter-gather).
    pub const CONTIGUOUS: Self = DmaMapFlags(1 << 2);
    /// Forced cache coherence (flush/invalidate around transfers).
    pub const CACHE_SYNC: Self = DmaMapFlags(1 << 3);
    /// Do not insert into IOMMU table (raw passthrough).
    pub const BYPASS_IOMMU: Self = DmaMapFlags(1 << 4);
    /// Permanent buffer (do not re-allocate at each transfer).
    pub const PERSISTENT: Self = DmaMapFlags(1 << 5);
    /// Read-only for device.
    pub const READ_ONLY: Self = DmaMapFlags(1 << 6);
    /// Write-only for device.
    pub const WRITE_ONLY: Self = DmaMapFlags(1 << 7);
    /// Invalidates cache after transfer -> Device (for_cpu flush).
    pub const SYNC_FOR_CPU: Self = DmaMapFlags(1 << 8);
    /// Flushes cache before transfer -> Device (for_device flush).
    pub const SYNC_FOR_DEV: Self = DmaMapFlags(1 << 9);

    #[inline]
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
    #[inline]
    pub fn set(self, other: Self) -> Self {
        DmaMapFlags(self.0 | other.0)
    }
    #[inline]
    pub fn clear(self, other: Self) -> Self {
        DmaMapFlags(self.0 & !other.0)
    }
}

impl core::ops::BitOr for DmaMapFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        DmaMapFlags(self.0 | rhs.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TRANSACTION STATE
// ─────────────────────────────────────────────────────────────────────────────

/// State of a DMA transaction.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(u8)]
pub enum DmaTransactionState {
    /// Free slot.
    Free = 0,
    /// Transaction created but not yet submitted.
    Pending = 1,
    /// Transaction submitted to controller.
    Submitted = 2,
    /// Transfer currently executing.
    Running = 3,
    /// Transfer completed successfully.
    Done = 4,
    /// Transfer completed with an error.
    Error = 5,
    /// Transaction cancelled.
    Cancelled = 6,
}

// ─────────────────────────────────────────────────────────────────────────────
// CHANNEL PRIORITY
// ─────────────────────────────────────────────────────────────────────────────

/// Priority of a DMA channel (for request scheduling).
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Debug)]
#[repr(u8)]
pub enum DmaPriority {
    Low = 0,
    Normal = 1,
    High = 2,
    Realtime = 3,
}

// ─────────────────────────────────────────────────────────────────────────────
// CHANNEL CAPABILITIES
// ─────────────────────────────────────────────────────────────────────────────

/// Capabilities advertised by a DMA channel (bitfield).
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(transparent)]
pub struct DmaCapabilities(pub u32);

impl DmaCapabilities {
    pub const MEMCPY: Self = DmaCapabilities(1 << 0);
    pub const MEMSET: Self = DmaCapabilities(1 << 1);
    pub const XOR: Self = DmaCapabilities(1 << 2);
    pub const PQ: Self = DmaCapabilities(1 << 3);
    pub const INTERRUPT: Self = DmaCapabilities(1 << 4);
    pub const SCATTER_GATHER: Self = DmaCapabilities(1 << 5);
    pub const CYCLIC: Self = DmaCapabilities(1 << 6);
    pub const INTERLEAVED: Self = DmaCapabilities(1 << 7);
    pub const SLAVE_SG: Self = DmaCapabilities(1 << 8);
    pub const PRIVATE: Self = DmaCapabilities(1 << 9);
    pub const ASYNC_TX: Self = DmaCapabilities(1 << 10);
    pub const REPEAT: Self = DmaCapabilities(1 << 11);
    pub const LOAD_EOT: Self = DmaCapabilities(1 << 12);
    pub const NONE: Self = DmaCapabilities(0);

    #[inline]
    pub fn has(self, cap: Self) -> bool {
        self.0 & cap.0 == cap.0
    }
    #[inline]
    pub fn set(self, cap: Self) -> Self {
        DmaCapabilities(self.0 | cap.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DMA ERRORS
// ─────────────────────────────────────────────────────────────────────────────

/// Specific errors for DMA subsystem.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(u8)]
pub enum DmaError {
    /// No DMA channel available.
    NoChannel = 0,
    /// Out of DMA memory.
    OutOfMemory = 1,
    /// Invalid parameters (size, alignment...).
    InvalidParams = 2,
    /// Transfer timeout.
    Timeout = 3,
    /// Hardware error (bus, parity...).
    HardwareError = 4,
    /// IOMMU error (mapping denied, page fault).
    IommuFault = 5,
    /// Channel not initialized.
    NotInitialized = 6,
    /// Transaction already submitted.
    AlreadySubmitted = 7,
    /// Transaction cancelled.
    Cancelled = 8,
    /// Buffer misaligned for required granularity.
    MisalignedBuffer = 9,
    /// Address not in required DMA zone.
    WrongZone = 10,
    /// Operation not supported by this channel.
    NotSupported = 11,
}
