// kernel/src/memory/dma/core/descriptor.rs
//
// DMA transaction descriptor — main structure manipulated by
// DMA engines and channels.
//
// LAYER 0 — zero external dependencies.

use crate::memory::core::types::PhysAddr;
use crate::memory::dma::core::types::{
    DmaDirection, DmaError, DmaMapFlags, DmaPriority, DmaTransactionId, DmaTransactionState,
    IovaAddr,
};
use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// SCATTER-GATHER
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum scatter-gather entries per transaction.
pub const MAX_SG_ENTRIES: usize = 32;

/// A scatter-gather entry: (physical_address, length).
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct SgEntry {
    /// Physical address of the fragment.
    pub phys: PhysAddr,
    /// Length of the fragment in bytes.
    pub len: u32,
    /// Offset relative to base page.
    pub page_offset: u16,
    /// Reserved.
    pub _pad: [u8; 2],
}

impl SgEntry {
    pub const EMPTY: Self = SgEntry {
        phys: PhysAddr::new(0),
        len: 0,
        page_offset: 0,
        _pad: [0u8; 2],
    };

    #[inline]
    pub fn total_bytes(&self) -> usize {
        self.len as usize
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MAIN DESCRIPTOR
// ─────────────────────────────────────────────────────────────────────────────

/// Full descriptor of a DMA transaction.
/// Target size: 256 bytes to fit into 4 cache lines.
#[repr(C, align(64))]
pub struct DmaDescriptor {
    // ── Identity ──────────────────────────────────────────────────────────────
    /// Unique transaction identifier.
    pub txn_id: DmaTransactionId,
    /// Associated DMA channel.
    pub channel_id: u32,
    /// Requesting thread/process (for completion wakeup).
    pub requester_tid: u64,

    // ── Transfer parameters ───────────────────────────────────────────────────
    /// Transfer direction.
    pub direction: DmaDirection,
    /// Priority.
    pub priority: DmaPriority,
    /// Mapping flags.
    pub map_flags: DmaMapFlags,
    /// IOMMU domain (0 = system domain).
    pub iommu_domain: u32,

    // ── Source ────────────────────────────────────────────────────────────────
    /// Source address (physical) — valid if src_sg_count == 0.
    pub src_phys: PhysAddr,
    /// Source IOVA mapped by IOMMU.
    pub src_iova: IovaAddr,
    /// Number of source SG entries (0 = single address).
    pub src_sg_count: u16,

    // ── Destination ───────────────────────────────────────────────────────────
    /// Destination address (physical) — valid if dst_sg_count == 0.
    pub dst_phys: PhysAddr,
    /// Destination IOVA mapped by IOMMU.
    pub dst_iova: IovaAddr,
    /// Number of destination SG entries (0 = single address).
    pub dst_sg_count: u16,

    // ── Size ──────────────────────────────────────────────────────────────────
    /// Total transfer size in bytes.
    pub transfer_size: usize,
    /// Bytes effectively transferred (updated by engine).
    pub bytes_done: AtomicU64,

    // ── State ─────────────────────────────────────────────────────────────────
    /// Current transaction state.
    pub state: AtomicU8,
    /// Error code (valid if state == Error).
    pub error: AtomicU8,
    /// Submission timestamp (TSC cycles).
    pub submit_tsc: AtomicU64,
    /// Completion timestamp (TSC cycles).
    pub done_tsc: AtomicU64,

    // ── Scatter-Gather ────────────────────────────────────────────────────────
    /// Source SG table.
    pub src_sg: [SgEntry; MAX_SG_ENTRIES],
    /// Destination SG table.
    pub dst_sg: [SgEntry; MAX_SG_ENTRIES],

    /// Padding to align to 64.
    _pad: [u8; 6],
}

// Size check: <= 2048 bytes (SG tables included, 2×32×16 = 1024 + headers).
const _: () = assert!(core::mem::size_of::<DmaDescriptor>() <= 2048);

impl DmaDescriptor {
    /// Constructs an empty descriptor with the provided ID.
    pub fn new(txn_id: DmaTransactionId, channel_id: u32, requester_tid: u64) -> Self {
        DmaDescriptor {
            txn_id,
            channel_id,
            requester_tid,
            direction: DmaDirection::Bidirection,
            priority: DmaPriority::Normal,
            map_flags: DmaMapFlags::NONE,
            iommu_domain: 0,
            src_phys: PhysAddr::new(0),
            src_iova: IovaAddr::zero(),
            src_sg_count: 0,
            dst_phys: PhysAddr::new(0),
            dst_iova: IovaAddr::zero(),
            dst_sg_count: 0,
            transfer_size: 0,
            bytes_done: AtomicU64::new(0),
            state: AtomicU8::new(DmaTransactionState::Pending as u8),
            error: AtomicU8::new(0),
            submit_tsc: AtomicU64::new(0),
            done_tsc: AtomicU64::new(0),
            src_sg: [SgEntry::EMPTY; MAX_SG_ENTRIES],
            dst_sg: [SgEntry::EMPTY; MAX_SG_ENTRIES],
            _pad: [0u8; 6],
        }
    }

    // ── State accessors ───────────────────────────────────────────────────────

    #[inline]
    pub fn state(&self) -> DmaTransactionState {
        match self.state.load(Ordering::Acquire) {
            0 => DmaTransactionState::Free,
            1 => DmaTransactionState::Pending,
            2 => DmaTransactionState::Submitted,
            3 => DmaTransactionState::Running,
            4 => DmaTransactionState::Done,
            5 => DmaTransactionState::Error,
            6 => DmaTransactionState::Cancelled,
            _ => DmaTransactionState::Error,
        }
    }

    #[inline]
    pub fn set_state(&self, s: DmaTransactionState) {
        self.state.store(s as u8, Ordering::Release);
    }

    #[inline]
    pub fn set_error(&self, e: DmaError) {
        self.error.store(e as u8, Ordering::Release);
        self.set_state(DmaTransactionState::Error);
    }

    #[inline]
    pub fn mark_done(&self, bytes: usize) {
        self.bytes_done.store(bytes as u64, Ordering::Release);
        self.set_state(DmaTransactionState::Done);
    }

    #[inline]
    pub fn is_done(&self) -> bool {
        matches!(
            self.state(),
            DmaTransactionState::Done | DmaTransactionState::Error | DmaTransactionState::Cancelled
        )
    }

    // ── SG Configuration ─────────────────────────────────────────────────────

    /// Adds a source SG entry.
    pub fn add_src_sg(&mut self, phys: PhysAddr, len: u32) -> bool {
        let idx = self.src_sg_count as usize;
        if idx >= MAX_SG_ENTRIES {
            return false;
        }
        self.src_sg[idx] = SgEntry {
            phys,
            len,
            page_offset: 0,
            _pad: [0; 2],
        };
        self.src_sg_count += 1;
        self.transfer_size += len as usize;
        true
    }

    /// Adds a destination SG entry.
    pub fn add_dst_sg(&mut self, phys: PhysAddr, len: u32) -> bool {
        let idx = self.dst_sg_count as usize;
        if idx >= MAX_SG_ENTRIES {
            return false;
        }
        self.dst_sg[idx] = SgEntry {
            phys,
            len,
            page_offset: 0,
            _pad: [0; 2],
        };
        self.dst_sg_count += 1;
        true
    }

    /// Configures a simple transfer (single src -> single dst).
    pub fn setup_simple(&mut self, src: PhysAddr, dst: PhysAddr, size: usize, dir: DmaDirection) {
        self.src_phys = src;
        self.dst_phys = dst;
        self.transfer_size = size;
        self.direction = dir;
        self.src_sg_count = 0;
        self.dst_sg_count = 0;
    }

    /// Configures a FILL transfer (DMA memset): fills `size` bytes at `dst`
    /// with the pattern `value`.
    ///
    /// Exo-OS convention: `src_phys` encodes fill value
    /// (bits [7:0]) for I/OAT/DSA engines using pattern fill.
    /// `src_sg_count == 0` and `src_iova == 0` indicate FILL mode.
    pub fn setup_fill(&mut self, dst: PhysAddr, value: u8, size: usize) {
        // Encode fill value into src_phys.0 (low 8 bits).
        self.src_phys = PhysAddr::new(value as u64);
        self.dst_phys = dst;
        self.transfer_size = size;
        self.direction = DmaDirection::ToDevice;
        self.src_sg_count = 0;
        self.dst_sg_count = 0;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TRANSACTION TABLE (FREE DESCRIPTOR RING BUFFER)
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum number of concurrent DMA transactions managed by the system.
pub const MAX_DMA_TRANSACTIONS: usize = 512;

/// Global table of reusable DMA descriptors.
pub struct DmaDescriptorTable {
    slots: spin::Mutex<DmaDescriptorTableInner>,
}

struct DmaDescriptorTableInner {
    free_list: [u16; MAX_DMA_TRANSACTIONS], // free indices
    free_head: usize,
    free_count: usize,
    /// Descriptor storage (MaybeUninit to avoid initialization).
    storage: [core::mem::MaybeUninit<DmaDescriptor>; MAX_DMA_TRANSACTIONS],
}

// SAFETY: DmaDescriptorTable is protected by a spin::Mutex.
unsafe impl Sync for DmaDescriptorTable {}
unsafe impl Send for DmaDescriptorTable {}

impl Default for DmaDescriptorTable {
    fn default() -> Self {
        Self::new()
    }
}

impl DmaDescriptorTable {
    const fn new() -> Self {
        DmaDescriptorTable {
            slots: spin::Mutex::new(DmaDescriptorTableInner {
                // Initialize free_list[i] = i.
                free_list: [0u16; MAX_DMA_TRANSACTIONS],
                free_head: 0,
                free_count: 0,
                storage: [const { core::mem::MaybeUninit::uninit() }; MAX_DMA_TRANSACTIONS],
            }),
        }
    }

    /// Initializes table (single call at boot).
    pub fn init(&self) {
        let mut inner = self.slots.lock();
        for i in 0..MAX_DMA_TRANSACTIONS {
            inner.free_list[i] = i as u16;
        }
        inner.free_head = 0;
        inner.free_count = MAX_DMA_TRANSACTIONS;
    }

    /// Allocates a descriptor from the pool.
    #[allow(clippy::mut_from_ref)]
    pub fn alloc_descriptor(
        &self,
        channel_id: u32,
        requester_tid: u64,
    ) -> Option<&mut DmaDescriptor> {
        let mut inner = self.slots.lock();
        if inner.free_count == 0 {
            return None;
        }

        let idx = inner.free_list[inner.free_head] as usize;
        inner.free_head = (inner.free_head + 1) % MAX_DMA_TRANSACTIONS;
        inner.free_count -= 1;

        let txn_id = DmaTransactionId::generate();
        let desc_ptr = inner.storage[idx].as_mut_ptr();
        // SAFETY: Slot is free (free_list guarantees uniqueness).
        unsafe {
            desc_ptr.write(DmaDescriptor::new(txn_id, channel_id, requester_tid));
            Some(&mut *(desc_ptr))
        }
    }

    /// Frees a descriptor (must only be called after is_done()).
    ///
    /// # Safety
    /// `idx` must be the descriptor index in storage.
    /// Descriptor must be in a terminal state (Done/Error/Cancelled).
    pub unsafe fn free_descriptor(&self, idx: usize) {
        let mut inner = self.slots.lock();
        let tail = (inner.free_head + inner.free_count) % MAX_DMA_TRANSACTIONS;
        inner.free_list[tail] = idx as u16;
        inner.free_count += 1;
    }
}

pub static DMA_DESCRIPTOR_TABLE: DmaDescriptorTable = DmaDescriptorTable::new();
