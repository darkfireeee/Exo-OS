// kernel/src/memory/dma/core/mapping.rs
//
// DMA mapping — translates physical addresses to IOVA (I/O Virtual Address)
// for DMA transfers with or without IOMMU.
//
// LAYER 0 — zero external dependencies.

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

use crate::memory::core::constants::PAGE_SIZE;
use crate::memory::core::types::PhysAddr;
use crate::memory::dma::core::descriptor::SgEntry;
use crate::memory::dma::core::types::{
    DmaDirection, DmaError, DmaMapFlags, IommuDomainId, IovaAddr,
};

// ─────────────────────────────────────────────────────────────────────────────
// IOVA ADDRESS SPACE
// ─────────────────────────────────────────────────────────────────────────────

/// Base of IOVA space (identical to phys in passthrough mode).
const IOVA_BASE: u64 = 0x0001_0000_0000; // By convention: beyond 4 GiB
const IOVA_SIZE: u64 = 0x0010_0000_0000; // 64 GiB of IOVA space

/// Name of legacy DMA zone (< 16 MiB).
const DMA16_LIMIT: u64 = 16 * 1024 * 1024;
const DMA32_LIMIT: u64 = 4 * 1024 * 1024 * 1024;

// ─────────────────────────────────────────────────────────────────────────────
// MAPPING DESCRIPTOR
// ─────────────────────────────────────────────────────────────────────────────

/// An active DMA mapping (physical -> IOVA).
#[repr(C, align(32))]
pub struct DmaMapping {
    /// Physical source address of the mapping.
    pub phys_base: PhysAddr,
    /// IOVA allocated for this mapping.
    pub iova_base: IovaAddr,
    /// Mapping size in bytes (multiple of PAGE_SIZE).
    pub size: usize,
    /// Transfer direction.
    pub direction: DmaDirection,
    /// Applied flags.
    pub flags: DmaMapFlags,
    /// Owning IOMMU domain.
    pub domain: IommuDomainId,
    /// Reference count (for shared persistent buffers).
    ref_count: AtomicUsize,
    /// Reserved.
    _pad: [u8; 4],
}

impl DmaMapping {
    fn new(
        phys_base: PhysAddr,
        iova_base: IovaAddr,
        size: usize,
        direction: DmaDirection,
        flags: DmaMapFlags,
        domain: IommuDomainId,
    ) -> Self {
        DmaMapping {
            phys_base,
            iova_base,
            size,
            direction,
            flags,
            domain,
            ref_count: AtomicUsize::new(1),
            _pad: [0u8; 4],
        }
    }

    #[inline]
    pub fn iova_end(&self) -> IovaAddr {
        IovaAddr::new(self.iova_base.as_u64() + self.size as u64)
    }
    #[inline]
    pub fn phys_end(&self) -> PhysAddr {
        PhysAddr::new(self.phys_base.as_u64() + self.size as u64)
    }
    #[inline]
    pub fn inc_ref(&self) -> usize {
        self.ref_count.fetch_add(1, Ordering::Relaxed) + 1
    }
    #[inline]
    pub fn dec_ref(&self) -> usize {
        self.ref_count.fetch_sub(1, Ordering::Release) - 1
    }
    #[inline]
    pub fn ref_count(&self) -> usize {
        self.ref_count.load(Ordering::Relaxed)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// IOVA ALLOCATOR (SIMPLE BUMP POINTER)
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum number of concurrent DMA mappings.
pub const MAX_DMA_MAPPINGS: usize = 4096;

struct IovaAllocatorInner {
    /// Next available IOVA (bump pointer, page-aligned).
    next: u64,
    /// Active mappings (flat table, O(n) but sufficient for boot).
    /// After IOMMU activation, IOMMU trees will be used.
    active: [Option<DmaMapping>; MAX_DMA_MAPPINGS],
    count: usize,
}

pub struct IovaAllocator {
    inner: Mutex<IovaAllocatorInner>,
    /// Passthrough mode: no IOMMU translation (IOVA == PhysAddr).
    passthrough: AtomicUsize, // 0=IOMMU active, 1=passthrough
    stats_allocs: AtomicU64,
    stats_frees: AtomicU64,
    stats_bytes: AtomicU64,
}

// SAFETY: IovaAllocator is protected by Mutex.
unsafe impl Sync for IovaAllocator {}
unsafe impl Send for IovaAllocator {}

impl IovaAllocator {
    const fn new() -> Self {
        const EMPTY_MAPPING: Option<DmaMapping> = None;

        IovaAllocator {
            inner: Mutex::new(IovaAllocatorInner {
                next: IOVA_BASE,
                active: [EMPTY_MAPPING; MAX_DMA_MAPPINGS],
                count: 0,
            }),
            passthrough: AtomicUsize::new(1), // passthrough by default (before IOMMU init)
            stats_allocs: AtomicU64::new(0),
            stats_frees: AtomicU64::new(0),
            stats_bytes: AtomicU64::new(0),
        }
    }

    /// Enables IOMMU mode (disables passthrough).
    pub fn enable_iommu(&self) {
        self.passthrough.store(0, Ordering::Release);
    }
    pub fn is_passthrough(&self) -> bool {
        self.passthrough.load(Ordering::Acquire) != 0
    }

    /// Maps `phys` -> IOVA of `size` bytes.
    ///
    /// In passthrough mode: IOVA == phys (no actual allocation).
    /// In IOMMU mode: bump pointer in [IOVA_BASE, IOVA_BASE+IOVA_SIZE).
    pub fn map(
        &self,
        phys: PhysAddr,
        size: usize,
        direction: DmaDirection,
        flags: DmaMapFlags,
        domain: IommuDomainId,
    ) -> Result<IovaAddr, DmaError> {
        // Validate alignment.
        if size == 0 {
            return Err(DmaError::InvalidParams);
        }
        let size_aligned = (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

        // Zone constraints.
        if flags.contains(DmaMapFlags::DMA16) && phys.as_u64() >= DMA16_LIMIT {
            return Err(DmaError::WrongZone);
        }
        if flags.contains(DmaMapFlags::DMA32) && phys.as_u64() >= DMA32_LIMIT {
            return Err(DmaError::WrongZone);
        }

        if self.is_passthrough() {
            // Passthrough mode: IOVA = PhysAddr directly.
            let iova = IovaAddr::new(phys.as_u64());
            self.stats_allocs.fetch_add(1, Ordering::Relaxed);
            self.stats_bytes
                .fetch_add(size_aligned as u64, Ordering::Relaxed);
            return Ok(iova);
        }

        // IOMMU mode: allocates an IOVA range, except when an explicit
        // physical DMA address is requested for a device still attached in
        // passthrough.
        let mut inner = self.inner.lock();
        if inner.count >= MAX_DMA_MAPPINGS {
            return Err(DmaError::OutOfMemory);
        }

        let iova = if flags.contains(DmaMapFlags::BYPASS_IOMMU) {
            IovaAddr::new(phys.as_u64())
        } else {
            let iova = IovaAddr::new(inner.next);
            inner.next = inner.next.wrapping_add(size_aligned as u64);
            if inner.next >= IOVA_BASE + IOVA_SIZE {
                // Wrap-around: reuse not supported yet.
                return Err(DmaError::OutOfMemory);
            }
            iova
        };

        // Insert into first free slot.
        let slot = inner
            .active
            .iter_mut()
            .find(|s| s.is_none())
            .ok_or(DmaError::OutOfMemory)?;
        *slot = Some(DmaMapping::new(
            phys,
            iova,
            size_aligned,
            direction,
            flags,
            domain,
        ));
        inner.count += 1;

        drop(inner);
        self.stats_allocs.fetch_add(1, Ordering::Relaxed);
        self.stats_bytes
            .fetch_add(size_aligned as u64, Ordering::Relaxed);
        Ok(iova)
    }

    /// Frees an IOVA mapping.
    pub fn unmap(&self, iova: IovaAddr, domain: IommuDomainId) -> Result<(), DmaError> {
        if self.is_passthrough() {
            self.stats_frees.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }

        let mut inner = self.inner.lock();
        for slot in inner.active.iter_mut() {
            if let Some(ref m) = slot {
                if m.iova_base == iova && m.domain == domain {
                    let rc = m.dec_ref();
                    if rc == 0 {
                        // Invalidate IOMMU TLB for this IOVA in the domain.
                        // SAFETY: CPL 0; flush_iotlb_domain verifies its own VT-d guard.
                        unsafe {
                            crate::memory::dma::iommu::intel_vtd::INTEL_VTD
                                .flush_iotlb_domain(domain.0 as u16, iova.0);
                        }
                        *slot = None;
                        inner.count -= 1;
                        drop(inner);
                        self.stats_frees.fetch_add(1, Ordering::Relaxed);
                        return Ok(());
                    }
                    return Ok(());
                }
            }
        }
        Err(DmaError::InvalidParams)
    }

    /// Maps a scatter-gather list into contiguous IOVA.
    pub fn map_sg(
        &self,
        entries: &[SgEntry],
        direction: DmaDirection,
        flags: DmaMapFlags,
        domain: IommuDomainId,
        out_iovas: &mut [IovaAddr],
    ) -> Result<usize, DmaError> {
        if entries.len() > out_iovas.len() {
            return Err(DmaError::InvalidParams);
        }
        let mut mapped = 0usize;
        for (entry, iova_out) in entries.iter().zip(out_iovas.iter_mut()) {
            if entry.is_empty() {
                break;
            }
            let iova = self.map(entry.phys, entry.len as usize, direction, flags, domain)?;
            *iova_out = iova;
            mapped += 1;
        }
        Ok(mapped)
    }

    /// Synchronizes CPU cache for a mapping (for_cpu or for_device).
    ///
    /// On x86_64 with coherent DMA (Intel / AMD-Vi): no-op.
    /// On ARM/RISC-V: would call cache flush instructions.
    #[inline]
    pub fn sync_for_cpu(&self, _iova: IovaAddr, _size: usize) {
        // x86_64: coherent cache with DMA — no action required.
        // A memory fence suffices for ordered visibility.
        core::sync::atomic::fence(Ordering::Acquire);
    }

    #[inline]
    pub fn sync_for_device(&self, _iova: IovaAddr, _size: usize) {
        core::sync::atomic::fence(Ordering::Release);
    }

    /// IOVA allocator statistics.
    pub fn stats(&self) -> (u64, u64, u64) {
        (
            self.stats_allocs.load(Ordering::Relaxed),
            self.stats_frees.load(Ordering::Relaxed),
            self.stats_bytes.load(Ordering::Relaxed),
        )
    }
}

/// Global IOVA allocator (system).
pub static IOVA_ALLOCATOR: IovaAllocator = IovaAllocator::new();
