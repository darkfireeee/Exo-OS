// kernel/src/memory/dma/iommu/domain.rs
//
// IOMMU domains — isolation between devices and address spaces.
// A domain groups a set of devices sharing the same
// IOVA space and the same IOMMU page tables.
//
// LAYER 0 — no external dependencies.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

use crate::memory::core::types::PhysAddr;
use crate::memory::dma::core::types::{DmaError, IommuDomainId, IovaAddr};

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTS
// ─────────────────────────────────────────────────────────────────────────────

pub const MAX_DOMAINS: usize = 256;
pub const MAX_DEVICES_PER_DOMAIN: usize = 64;
/// Identity domain ID (passthrough — no translation).
pub const IDENTITY_DOMAIN_ID: IommuDomainId = IommuDomainId(0);

// ─────────────────────────────────────────────────────────────────────────────
// DOMAIN TYPES
// ─────────────────────────────────────────────────────────────────────────────

/// IOMMU domain type.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(u8)]
pub enum DomainType {
    /// Identity: IOVA == PhysAddr (total passthrough).
    Identity = 0,
    /// Full translation via IOMMU page tables.
    Translated = 1,
    /// Partial translation (host + selective passthrough).
    Hybrid = 2,
    /// Blocked domain: all DMA accesses are denied.
    Blocked = 3,
}

// ─────────────────────────────────────────────────────────────────────────────
// DEVICE IDENTIFIER (BDF)
// ─────────────────────────────────────────────────────────────────────────────

/// PCI identifier of a device (Bus:Device:Function).
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(C)]
pub struct PciBdf {
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
    _pad: u8,
}

impl PciBdf {
    pub const fn new(bus: u8, dev: u8, func: u8) -> Self {
        PciBdf {
            bus,
            dev,
            func,
            _pad: 0,
        }
    }
    /// Encodes BDF as a 16-bit value (bus<<8 | dev<<3 | func).
    pub const fn as_u16(self) -> u16 {
        ((self.bus as u16) << 8) | ((self.dev as u16) << 3) | (self.func as u16)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// IOMMU DOMAIN
// ─────────────────────────────────────────────────────────────────────────────

/// IOMMU domain statistics.
pub struct DomainStats {
    pub mappings_created: AtomicU64,
    pub mappings_freed: AtomicU64,
    pub page_faults: AtomicU64,
    pub tlb_invalidations: AtomicU64,
}

impl DomainStats {
    const fn new() -> Self {
        DomainStats {
            mappings_created: AtomicU64::new(0),
            mappings_freed: AtomicU64::new(0),
            page_faults: AtomicU64::new(0),
            tlb_invalidations: AtomicU64::new(0),
        }
    }
}

/// An IOMMU domain.
#[repr(C, align(64))]
pub struct IommuDomain {
    /// Unique identifier.
    pub id: IommuDomainId,
    /// Domain type.
    pub domain_type: DomainType,
    /// Physical address of the IOMMU page table (PML4 or equivalent).
    pub page_table: AtomicU64,
    /// Active?
    pub active: AtomicBool,
    /// Number of attached devices.
    pub device_count: AtomicU32,
    /// BDF devices attached to this domain.
    devices: Mutex<[Option<PciBdf>; MAX_DEVICES_PER_DOMAIN]>,
    /// Statistics.
    pub stats: DomainStats,
    /// IOVA allocation counter in this domain.
    pub iova_counter: AtomicU64,
    /// IOVA base of this domain.
    pub iova_base: u64,
    /// Max size of IOVA space.
    pub iova_limit: u64,
    _pad: [u8; 4],
}

impl IommuDomain {
    const fn new_identity() -> Self {
        IommuDomain {
            id: IDENTITY_DOMAIN_ID,
            domain_type: DomainType::Identity,
            page_table: AtomicU64::new(0),
            active: AtomicBool::new(true),
            device_count: AtomicU32::new(0),
            devices: Mutex::new([None; MAX_DEVICES_PER_DOMAIN]),
            stats: DomainStats::new(),
            iova_counter: AtomicU64::new(0),
            iova_base: 0,
            iova_limit: u64::MAX,
            _pad: [0u8; 4],
        }
    }

    pub fn new(
        id: IommuDomainId,
        domain_type: DomainType,
        iova_base: u64,
        iova_limit: u64,
    ) -> Self {
        IommuDomain {
            id,
            domain_type,
            page_table: AtomicU64::new(0),
            active: AtomicBool::new(false),
            device_count: AtomicU32::new(0),
            devices: Mutex::new([None; MAX_DEVICES_PER_DOMAIN]),
            stats: DomainStats::new(),
            iova_counter: AtomicU64::new(iova_base),
            iova_base,
            iova_limit,
            _pad: [0u8; 4],
        }
    }

    /// Attaches a device to this domain.
    pub fn attach_device(&self, bdf: PciBdf) -> Result<(), DmaError> {
        let mut devs = self.devices.lock();
        if let Some(slot) = devs.iter_mut().find(|s| s.is_none()) {
            *slot = Some(bdf);
            self.device_count.fetch_add(1, Ordering::Relaxed);
            Ok(())
        } else {
            Err(DmaError::OutOfMemory)
        }
    }

    /// Detaches a device.
    pub fn detach_device(&self, bdf: PciBdf) {
        let mut devs = self.devices.lock();
        for slot in devs.iter_mut() {
            if *slot == Some(bdf) {
                *slot = None;
                self.device_count.fetch_sub(1, Ordering::Relaxed);
                return;
            }
        }
    }

    /// Allocates an IOVA in this domain (bump + alignment on PAGE_SIZE).
    pub fn alloc_iova(&self, size: usize) -> Option<IovaAddr> {
        use crate::memory::core::constants::PAGE_SIZE;
        let size_aligned = ((size + PAGE_SIZE - 1) / PAGE_SIZE) * PAGE_SIZE;
        let old = self.iova_counter.load(Ordering::Relaxed);
        let new = old + size_aligned as u64;
        if new > self.iova_limit {
            return None;
        }
        // CAS for atomicity.
        match self
            .iova_counter
            .compare_exchange(old, new, Ordering::AcqRel, Ordering::Relaxed)
        {
            Ok(_) => Some(IovaAddr::new(old)),
            Err(_) => {
                // Simple retry — under contention increment directly.
                let base = self
                    .iova_counter
                    .fetch_add(size_aligned as u64, Ordering::Relaxed);
                if base + size_aligned as u64 > self.iova_limit {
                    return None;
                }
                Some(IovaAddr::new(base))
            }
        }
    }

    /// Returns the physical address of the IOMMU page table.
    pub fn page_table_phys(&self) -> Option<PhysAddr> {
        let p = self.page_table.load(Ordering::Acquire);
        if p == 0 {
            None
        } else {
            Some(PhysAddr::new(p))
        }
    }

    /// Sets the page table (unsafe — must be called from IOMMU init).
    ///
    /// # Safety
    /// `phys` must point to a valid IOMMU page table aligned on PAGE_SIZE.
    pub unsafe fn set_page_table(&self, phys: PhysAddr) {
        self.page_table.store(phys.as_u64(), Ordering::Release);
    }

    pub fn activate(&self) {
        self.active.store(true, Ordering::Release);
    }
    pub fn deactivate(&self) {
        self.active.store(false, Ordering::Release);
    }
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GLOBAL DOMAIN TABLE
// ─────────────────────────────────────────────────────────────────────────────

pub struct IommuDomainTable {
    domains: Mutex<IommuDomainTableInner>,
    count: AtomicU32,
}

struct IommuDomainTableInner {
    slots: [Option<IommuDomain>; MAX_DOMAINS],
}

// SAFETY: IommuDomainTable is protected by Mutex.
unsafe impl Sync for IommuDomainTable {}
unsafe impl Send for IommuDomainTable {}

impl Default for IommuDomainTable {
    fn default() -> Self {
        Self::new()
    }
}

impl IommuDomainTable {
    const fn new() -> Self {
        const EMPTY_DOMAIN_SLOT: Option<IommuDomain> = None;

        IommuDomainTable {
            domains: Mutex::new(IommuDomainTableInner {
                slots: [EMPTY_DOMAIN_SLOT; MAX_DOMAINS],
            }),
            count: AtomicU32::new(0),
        }
    }

    /// Initializes the identity domain.
    pub fn init(&self) {
        let mut inner = self.domains.lock();
        inner.slots[0] = Some(IommuDomain::new_identity());
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    /// Creates a new translated domain.
    pub fn create_domain(
        &self,
        domain_type: DomainType,
        iova_base: u64,
        iova_limit: u64,
    ) -> Result<IommuDomainId, DmaError> {
        let mut inner = self.domains.lock();
        if self.count.load(Ordering::Relaxed) >= MAX_DOMAINS as u32 {
            return Err(DmaError::OutOfMemory);
        }

        // Reusing the slot index as DomainId guarantees a bounded and recyclable
        // identifier, compatible with static tables indexed by domain_id.
        let (slot_idx, slot) = inner
            .slots
            .iter_mut()
            .enumerate()
            .skip(1)
            .find(|(_, slot)| slot.is_none())
            .ok_or(DmaError::OutOfMemory)?;

        let id = IommuDomainId(slot_idx as u32);
        *slot = Some(IommuDomain::new(id, domain_type, iova_base, iova_limit));
        self.count.fetch_add(1, Ordering::Relaxed);
        Ok(id)
    }

    /// Destroys a translated domain and frees its slot for reuse.
    pub fn destroy_domain(&self, id: IommuDomainId) -> bool {
        if id == IDENTITY_DOMAIN_ID || id.0 as usize >= MAX_DOMAINS {
            return false;
        }

        let mut inner = self.domains.lock();
        let Some(slot) = inner.slots.get_mut(id.0 as usize) else {
            return false;
        };
        if slot.is_none() {
            return false;
        }

        *slot = None;
        self.count.fetch_sub(1, Ordering::Relaxed);
        true
    }

    /// Read-only access to a domain.
    pub fn with_domain<T, F: FnOnce(&IommuDomain) -> T>(
        &self,
        id: IommuDomainId,
        f: F,
    ) -> Option<T> {
        let inner = self.domains.lock();
        for dom in inner.slots.iter().flatten() {
            if dom.id == id {
                return Some(f(dom));
            }
        }
        None
    }

    /// Mutable access to a domain (for initialization).
    pub fn with_domain_mut<T, F: FnOnce(&mut IommuDomain) -> T>(
        &self,
        id: IommuDomainId,
        f: F,
    ) -> Option<T> {
        let mut inner = self.domains.lock();
        for dom in inner.slots.iter_mut().flatten() {
            if dom.id == id {
                return Some(f(dom));
            }
        }
        None
    }

    pub fn domain_count(&self) -> u32 {
        self.count.load(Ordering::Relaxed)
    }
}

pub static IOMMU_DOMAINS: IommuDomainTable = IommuDomainTable::new();
