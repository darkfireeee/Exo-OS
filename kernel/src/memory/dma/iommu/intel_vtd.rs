// kernel/src/memory/dma/iommu/intel_vtd.rs
//
// Intel VT-d (Virtualization Technology for Directed I/O) driver.
// Implements initialization and management of Intel remapping tables.
//
// LAYER 0 — no external dependencies.
// Reference: Intel VT-d Architecture Specification, Rev 3.4.

use core::sync::atomic::{compiler_fence, AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::memory::core::types::PhysAddr;
use crate::memory::dma::core::types::DmaError;

// ─────────────────────────────────────────────────────────────────────────────
// VT-d REGISTERS (offsets in DMAR MMIO)
// ─────────────────────────────────────────────────────────────────────────────

pub mod vtd_regs {
    /// Version register.
    pub const VER: usize = 0x000;
    /// Capability register.
    pub const CAP: usize = 0x008;
    /// Extended capability register.
    pub const ECAP: usize = 0x010;
    /// Global command register.
    pub const GCMD: usize = 0x018;
    /// Global status register.
    pub const GSTS: usize = 0x01C;
    /// Root table address register.
    pub const RTADDR: usize = 0x020;
    /// Context command register.
    pub const CCMD: usize = 0x028;
    /// Fault status register.
    pub const FSTS: usize = 0x034;
    /// Fault event control register.
    pub const FECTL: usize = 0x038;
    /// Invalidation queue head register.
    pub const IQH: usize = 0x080;
    /// Invalidation queue tail register.
    pub const IQT: usize = 0x088;
    /// Invalidation queue address register.
    pub const IQA: usize = 0x090;
    /// Invalidation completion status register.
    pub const ICS: usize = 0x09C;
}

/// GCMD register bits.
pub mod gcmd_bits {
    pub const TE: u32 = 1 << 31; // Translation Enable
    pub const SRTP: u32 = 1 << 30; // Set Root Table Pointer
    pub const SFL: u32 = 1 << 28; // Set Fault Log
    pub const QIE: u32 = 1 << 26; // Queued Invalidation Enable
    pub const SIRTP: u32 = 1 << 24; // Set Interrupt Remapping Table Pointer
    pub const IRE: u32 = 1 << 25; // Interrupt Remapping Enable
}

// ─────────────────────────────────────────────────────────────────────────────
// ROOT TABLE & CONTEXT TABLE (Intel format)
// ─────────────────────────────────────────────────────────────────────────────

/// Root Table entry (256 entries, one per PCI bus).
/// Each entry points to a Context Table of 256 entries.
#[repr(C, align(16))]
#[derive(Copy, Clone)]
pub struct RootEntry {
    pub lo: u64,
    pub hi: u64,
}

impl RootEntry {
    pub const EMPTY: Self = RootEntry { lo: 0, hi: 0 };
    pub const PRESENT: u64 = 1;

    #[inline]
    pub fn is_present(self) -> bool {
        self.lo & Self::PRESENT != 0
    }
    #[inline]
    pub fn context_table_phys(self) -> PhysAddr {
        PhysAddr::new(self.lo & 0xFFFF_FFFF_FFFF_F000)
    }
    #[inline]
    pub fn set_context_table(&mut self, phys: PhysAddr) {
        self.lo = phys.as_u64() | Self::PRESENT;
        self.hi = 0;
    }
}

/// The Root Table = 4096 bytes = 256 × RootEntry.
#[repr(C, align(4096))]
pub struct RootTable {
    pub entries: [RootEntry; 256],
}

impl RootTable {
    pub fn zero(&mut self) {
        // SAFETY: RootEntry is Copy and zero-initializable.
        unsafe {
            core::ptr::write_bytes(self.entries.as_mut_ptr(), 0, 256);
        }
        compiler_fence(Ordering::SeqCst);
    }
}

/// Context Table entry (one per device:function).
#[repr(C, align(16))]
#[derive(Copy, Clone)]
pub struct ContextEntry {
    pub lo: u64,
    pub hi: u64,
}

impl ContextEntry {
    pub const EMPTY: Self = ContextEntry { lo: 0, hi: 0 };
    pub const PRESENT: u64 = 1;
    /// Translation type: 00 = untranslated, 01 = translated, 10 = passthrough.
    pub const TT_TRANSLATED: u64 = 0b00 << 2;
    pub const TT_PASSTHROUGH: u64 = 0b10 << 2;

    #[inline]
    pub fn is_present(self) -> bool {
        self.lo & Self::PRESENT != 0
    }

    /// Configures for a translated domain.
    pub fn set_translated(&mut self, sl_phys: PhysAddr, domain_id: u16, aw: u8) {
        // AW (Address Width): 2=48-bit (4-level), 3=57-bit (5-level).
        // lo[63:12] = SL root table ptr, lo[3:2] = TT, lo[0] = P
        self.lo = sl_phys.as_u64() | Self::TT_TRANSLATED | Self::PRESENT;
        // hi[23:8] = Domain ID, hi[2:0] = AW
        self.hi = ((domain_id as u64) << 8) | (aw as u64 & 0x7);
    }

    pub fn set_passthrough(&mut self) {
        self.lo = Self::TT_PASSTHROUGH | Self::PRESENT;
        self.hi = 0;
    }
}

/// Context Table = 4096 bytes = 256 × ContextEntry.
#[repr(C, align(4096))]
pub struct ContextTable {
    pub entries: [ContextEntry; 256],
}

impl ContextTable {
    pub fn zero(&mut self) {
        // SAFETY: self.entries is a contiguous array of 256 ContextEntry; write_bytes cleanly overwrites it.
        unsafe {
            core::ptr::write_bytes(self.entries.as_mut_ptr(), 0, 256);
        }
        compiler_fence(Ordering::SeqCst);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// VT-d CONTROLLER
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum number of supported remapping hardware units.
pub const MAX_DRHD: usize = 8;

/// State of a VT-d hardware unit.
pub struct DmarUnit {
    /// Base MMIO address of this DRHD.
    pub mmio_base: u64,
    /// Size of the MMIO region (typically 4096).
    pub mmio_size: u64,
    /// Capabilities (CAP register).
    pub cap: u64,
    /// Extended capabilities (ECAP register).
    pub ecap: u64,
    /// Physical Root Table for this unit.
    pub root_table_phys: AtomicU64,
    /// Enabled?
    pub enabled: AtomicBool,
    /// Unit initialized?
    pub initialized: AtomicBool,
}

impl DmarUnit {
    const fn new_uninit() -> Self {
        DmarUnit {
            mmio_base: 0,
            mmio_size: 0,
            cap: 0,
            ecap: 0,
            root_table_phys: AtomicU64::new(0),
            enabled: AtomicBool::new(false),
            initialized: AtomicBool::new(false),
        }
    }

    /// Reads a 32-bit DMAR MMIO register.
    ///
    /// # Safety
    /// `mmio_base` must be mapped and accessible.
    unsafe fn read32(&self, offset: usize) -> u32 {
        let ptr = (self.mmio_base + offset as u64) as *const u32;
        core::ptr::read_volatile(ptr)
    }

    /// Writes a 32-bit DMAR MMIO register.
    ///
    /// # Safety
    /// `mmio_base` must be mapped and accessible.
    unsafe fn write32(&self, offset: usize, val: u32) {
        let ptr = (self.mmio_base + offset as u64) as *mut u32;
        core::ptr::write_volatile(ptr, val);
    }

    /// Writes a 64-bit DMAR MMIO register.
    ///
    /// # Safety
    /// `mmio_base` must be mapped and accessible. Requires a 64-bit write.
    unsafe fn write64(&self, offset: usize, val: u64) {
        let ptr = (self.mmio_base + offset as u64) as *mut u64;
        core::ptr::write_volatile(ptr, val);
    }

    /// Initializes this DRHD unit (reads capabilities).
    ///
    /// # Safety
    /// `mmio_base` is a valid virtual address mapped to DMAR MMIO.
    pub unsafe fn init(&mut self, mmio_base: u64, mmio_size: u64) {
        self.mmio_base = mmio_base;
        self.mmio_size = mmio_size;
        self.cap =
            (self.read32(vtd_regs::CAP) as u64) | ((self.read32(vtd_regs::CAP + 4) as u64) << 32);
        self.ecap =
            (self.read32(vtd_regs::ECAP) as u64) | ((self.read32(vtd_regs::ECAP + 4) as u64) << 32);
    }

    /// Configures the Root Table for this DRHD.
    ///
    /// # Safety
    /// The Root Table must be allocated and zero-initialized.
    pub unsafe fn set_root_table(&self, rt_phys: PhysAddr) {
        // Write the Root Table address.
        self.write64(vtd_regs::RTADDR, rt_phys.as_u64());
        // Set Root Table Pointer.
        self.write32(vtd_regs::GCMD, gcmd_bits::SRTP);
        // Wait for hardware confirmation (poll on GSTS.RTPS).
        let mut timeout = 1_000_000u32;
        while timeout > 0 {
            let gsts = self.read32(vtd_regs::GSTS);
            if gsts & (1 << 30) != 0 {
                break;
            } // RTPS bit
            timeout -= 1;
        }
        self.root_table_phys
            .store(rt_phys.as_u64(), Ordering::Release);
    }

    /// Enables DMA translation on this DRHD.
    ///
    /// # Safety
    /// The Root Table must be properly configured before activation.
    pub unsafe fn enable_translation(&self) {
        self.write32(vtd_regs::GCMD, gcmd_bits::TE);
        let mut timeout = 1_000_000u32;
        while timeout > 0 {
            let gsts = self.read32(vtd_regs::GSTS);
            if gsts & (1 << 31) != 0 {
                break;
            } // TES bit
            timeout -= 1;
        }
        self.enabled.store(true, Ordering::Release);
    }

    /// Invalidates context cache for a device (BDF).
    ///
    /// # Safety
    /// Requires the DRHD to be enabled.
    pub unsafe fn invalidate_context(&self, domain_id: u16, bus: u8, df: u8) {
        // CCMD: IWS=1, CIIG=00 (device-specific), DID, SID
        let ccmd: u64 = (1u64 << 63)              // ICC
                      | ((domain_id as u64) << 16)
                      | ((bus as u64) << 8)
                      | (df as u64);
        self.write64(vtd_regs::CCMD, ccmd);
        // Wait for ICC = 0.
        let mut timeout = 1_000_000u32;
        while timeout > 0 {
            let val =
                (self.read32(vtd_regs::CCMD + 4) as u64) << 32 | self.read32(vtd_regs::CCMD) as u64;
            if val & (1u64 << 63) == 0 {
                break;
            }
            timeout -= 1;
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GLOBAL VT-d MANAGER
// ─────────────────────────────────────────────────────────────────────────────

pub struct IntelVtd {
    units: [DmarUnit; MAX_DRHD],
    count: AtomicU32,
    initialized: AtomicBool,
}

// SAFETY: access protected by AtomicBool + init order (single-threaded init).
unsafe impl Sync for IntelVtd {}
unsafe impl Send for IntelVtd {}

impl IntelVtd {
    const fn new() -> Self {
        IntelVtd {
            units: [
                DmarUnit::new_uninit(),
                DmarUnit::new_uninit(),
                DmarUnit::new_uninit(),
                DmarUnit::new_uninit(),
                DmarUnit::new_uninit(),
                DmarUnit::new_uninit(),
                DmarUnit::new_uninit(),
                DmarUnit::new_uninit(),
            ],
            count: AtomicU32::new(0),
            initialized: AtomicBool::new(false),
        }
    }

    /// Registers a hardware DRHD unit.
    ///
    /// # Safety
    /// Called from the ACPI DMAR parser, before `global_init()`.
    pub unsafe fn register_drhd(&self, mmio_base: u64, mmio_size: u64) -> Result<(), DmaError> {
        let idx = self.count.load(Ordering::Relaxed) as usize;
        if idx >= MAX_DRHD {
            return Err(DmaError::OutOfMemory);
        }

        let unit_ptr = &self.units[idx] as *const DmarUnit as *mut DmarUnit;
        (*unit_ptr).init(mmio_base, mmio_size);
        self.count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Globally initializes VT-d (after registering all DRHDs).
    ///
    /// # Safety
    /// Called only once at boot.
    pub unsafe fn global_init(&self) {
        self.initialized.store(true, Ordering::Release);
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }

    pub fn unit_count(&self) -> usize {
        self.count.load(Ordering::Relaxed) as usize
    }

    /// Invalidates the IOTLB entry for the given IOVA in domain `domain_id`
    /// across all initialized DRHD units.
    ///
    /// Uses the Register-Based Invalidation Interface (CCMD + IOTLB_REG),
    /// a synchronous method guaranteeing consistency before returning.
    ///
    /// # Safety
    /// CPL 0, VT-d initialized.
    pub unsafe fn flush_iotlb_domain(&self, domain_id: u16, _iova: u64) {
        if !self.initialized.load(Ordering::Acquire) {
            return;
        }
        let count = self.count.load(Ordering::Relaxed) as usize;
        for i in 0..count {
            let unit = &self.units[i];
            if !unit.initialized.load(Ordering::Acquire) {
                continue;
            }

            // Context invalidation (domain-global) via CCMD.
            // DI = 0b01 = Domain-Selective invalidation.
            let ccmd: u64 = (1u64 << 63)              // ICC (Invalidation Context Cache)
                          | (0b01u64 << 61)            // Cireg = 01 (domain-selective)
                          | ((domain_id as u64) << 16);
            unit.write64(vtd_regs::CCMD, ccmd);

            // Wait for context invalidation completion (ICC → 0).
            let mut timeout = 1_000_000u32;
            while timeout > 0 {
                let hi = unit.read32(vtd_regs::CCMD + 4) as u64;
                if hi & (1 << 31) == 0 {
                    break;
                } // ICC cleared
                timeout -= 1;
            }

            // IOTLB invalidation: IOTLB_REG typically at ECAP[12:10]*16 + 8.
            // Here standard offset 0x108 (domain-selective) is used.
            // DI = 0b010 (IOTLB Domain-Selective), DID = domain_id.
            const IOTLB_REG_HI: usize = 0x10C;
            let iotlb_cmd: u32 = (1u32 << 31)                  // IVT = 1 (invalidate)
                                | (0b010u32 << 4)               // IIRG = 010 = domain
                                | ((domain_id as u32) << 16); // domain id
            unit.write32(IOTLB_REG_HI, iotlb_cmd);

            // Wait for completion (IVT → 0).
            let mut timeout = 1_000_000u32;
            while timeout > 0 {
                if unit.read32(IOTLB_REG_HI) & (1 << 31) == 0 {
                    break;
                }
                timeout -= 1;
            }
        }
    }
}

pub static INTEL_VTD: IntelVtd = IntelVtd::new();
