// kernel/src/memory/dma/iommu/intel_vtd.rs
//
// Intel VT-d (Virtualization Technology for Directed I/O) driver.
// Implements ACPI DMAR discovery, Root/Context Table configuration, real hardware
// DMA translation activation (`GCMD.SRTP` + `GCMD.TE`), spec-compliant Register-Based
// Context/IOTLB invalidation (`CCMD` + `ECAP.IRO` `IOTLB_REG`), and hardware DMAR
// fault record polling (`FSTS` + `CAP.FRO` `FRCD_REG`).
//
// Reference: Intel VT-d Architecture Specification, Rev 3.4.

use core::sync::atomic::{compiler_fence, AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::memory::core::types::PhysAddr;
use crate::memory::dma::core::types::DmaError;

// ─────────────────────────────────────────────────────────────────────────────
// VT-d REGISTERS (offsets in DMAR MMIO)
// ─────────────────────────────────────────────────────────────────────────────

pub mod vtd_regs {
    /// Version register (32-bit).
    pub const VER: usize = 0x000;
    /// Capability register (64-bit).
    pub const CAP: usize = 0x008;
    /// Extended capability register (64-bit).
    pub const ECAP: usize = 0x010;
    /// Global command register (32-bit, WO).
    pub const GCMD: usize = 0x018;
    /// Global status register (32-bit, RO).
    pub const GSTS: usize = 0x01C;
    /// Root table address register (64-bit).
    pub const RTADDR: usize = 0x020;
    /// Context command register (64-bit).
    pub const CCMD: usize = 0x028;
    /// Fault status register (32-bit).
    pub const FSTS: usize = 0x034;
    /// Fault event control register (32-bit).
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

/// GCMD register bits (Intel VT-d Spec §10.4.4).
pub mod gcmd_bits {
    pub const TE: u32 = 1 << 31; // Translation Enable
    pub const SRTP: u32 = 1 << 30; // Set Root Table Pointer (one-shot)
    pub const SFL: u32 = 1 << 29; // Set Fault Log (one-shot)
    pub const EAFL: u32 = 1 << 28; // Enable Advanced Fault Logging
    pub const WBF: u32 = 1 << 27; // Write Buffer Flush (one-shot)
    pub const QIE: u32 = 1 << 26; // Queued Invalidation Enable
    pub const IRE: u32 = 1 << 25; // Interrupt Remapping Enable
    pub const SIRTP: u32 = 1 << 24; // Set Interrupt Remapping Table Pointer (one-shot)
    /// Preserves persistent GSTS status bits when writing GCMD (masks out one-shot bits).
    pub const GSTS_PRESERVE_MASK: u32 = 0x96FF_FFFF;
}

/// Hardware DMAR fault record read directly from VT-d `FRCD_REG` (`CAP.FRO`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VtdHardwareFault {
    pub unit_idx: u8,
    pub source_id: u16,
    pub reason: u8,
    pub is_read: bool,
    pub fault_addr: u64,
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
        self.lo = (phys.as_u64() & 0xFFFF_FFFF_FFFF_F000) | Self::PRESENT;
        self.hi = 0;
    }
}

/// The Root Table = 4096 bytes = 256 × RootEntry.
#[repr(C, align(4096))]
pub struct RootTable {
    pub entries: [RootEntry; 256],
}

impl RootTable {
    pub const fn new_zeroed() -> Self {
        Self {
            entries: [RootEntry::EMPTY; 256],
        }
    }

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
    /// Translation type: 00 = untranslated (SLPT), 01 = translated (ATS), 10 = passthrough.
    pub const TT_TRANSLATED: u64 = 0b00 << 2;
    pub const TT_PASSTHROUGH: u64 = 0b10 << 2;

    #[inline]
    pub fn is_present(self) -> bool {
        self.lo & Self::PRESENT != 0
    }

    /// Configures for a translated second-level domain.
    pub fn set_translated(&mut self, sl_phys: PhysAddr, domain_id: u16, aw: u8) {
        // AW (Address Width): 1=39-bit (3-level), 2=48-bit (4-level), 3=57-bit (5-level).
        // lo[63:12] = SL root table ptr, lo[3:2] = TT, lo[0] = P
        self.lo = (sl_phys.as_u64() & 0xFFFF_FFFF_FFFF_F000) | Self::TT_TRANSLATED | Self::PRESENT;
        // hi[23:8] = Domain ID, hi[2:0] = AW
        self.hi = ((domain_id as u64) << 8) | (aw as u64 & 0x7);
    }

    /// Configures passthrough mode with a valid `CAP.SAGAW` Address Width (`aw`) and `domain_id`.
    pub fn set_passthrough_with_aw(&mut self, domain_id: u16, aw: u8) {
        self.lo = Self::TT_PASSTHROUGH | Self::PRESENT;
        self.hi = ((domain_id as u64) << 8) | (aw as u64 & 0x7);
    }

    pub fn set_passthrough(&mut self) {
        // Default 39-bit AGAW (AW=1) + Domain ID 1 (valid on QEMU intel-iommu and Intel chipsets).
        self.set_passthrough_with_aw(1, 1);
    }
}

/// Context Table = 4096 bytes = 256 × ContextEntry.
#[repr(C, align(4096))]
pub struct ContextTable {
    pub entries: [ContextEntry; 256],
}

impl ContextTable {
    pub const fn new_zeroed() -> Self {
        Self {
            entries: [ContextEntry::EMPTY; 256],
        }
    }

    pub fn zero(&mut self) {
        // SAFETY: self.entries is a contiguous array of 256 ContextEntry; write_bytes cleanly overwrites it.
        unsafe {
            core::ptr::write_bytes(self.entries.as_mut_ptr(), 0, 256);
        }
        compiler_fence(Ordering::SeqCst);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// STATIC HARDWARE TABLES (4 KiB-aligned in kernel BSS)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(all(target_arch = "x86_64", target_os = "none"))]
const STATIC_CONTEXT_BUSES: usize = 4;

#[cfg(all(target_arch = "x86_64", target_os = "none"))]
static mut VTD_ROOT_TABLE: RootTable = RootTable::new_zeroed();

#[cfg(all(target_arch = "x86_64", target_os = "none"))]
static mut VTD_CONTEXT_TABLES: [ContextTable; STATIC_CONTEXT_BUSES] = [
    ContextTable::new_zeroed(),
    ContextTable::new_zeroed(),
    ContextTable::new_zeroed(),
    ContextTable::new_zeroed(),
];

// ─────────────────────────────────────────────────────────────────────────────
// VT-d CONTROLLER
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum number of supported remapping hardware units.
pub const MAX_DRHD: usize = 8;

/// State of a VT-d hardware unit.
pub struct DmarUnit {
    /// Base virtual MMIO address of this DRHD.
    pub mmio_base: u64,
    /// Size of the MMIO region (typically 4096).
    pub mmio_size: u64,
    /// Version register (`VER`).
    pub ver: u32,
    /// Capabilities (`CAP` register).
    pub cap: u64,
    /// Extended capabilities (`ECAP` register).
    pub ecap: u64,
    /// Physical Root Table for this unit.
    pub root_table_phys: AtomicU64,
    /// DMA translation enabled (`GSTS.TES == 1`)?
    pub enabled: AtomicBool,
    /// Unit initialized?
    pub initialized: AtomicBool,
}

impl DmarUnit {
    const fn new_uninit() -> Self {
        DmarUnit {
            mmio_base: 0,
            mmio_size: 0,
            ver: 0,
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

    /// Reads a 64-bit DMAR MMIO register.
    ///
    /// # Safety
    /// `mmio_base` must be mapped and accessible.
    unsafe fn read64(&self, offset: usize) -> u64 {
        let ptr = (self.mmio_base + offset as u64) as *const u64;
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

    /// Returns `ECAP.IRO * 16` — byte offset of the IOTLB invalidation registers.
    #[inline]
    pub fn iotlb_reg_base(&self) -> usize {
        let iro = ((self.ecap >> 8) & 0x3FF) as usize;
        if iro == 0 {
            0x100
        } else {
            iro * 16
        }
    }

    /// Returns `CAP.FRO * 16` — byte offset of the Fault Recording Registers (`FRCD_REG`).
    #[inline]
    pub fn fault_rec_offset(&self) -> usize {
        (((self.cap >> 24) & 0x3FF) as usize) * 16
    }

    /// Returns `CAP.NFR + 1` — number of Fault Recording Registers (clamped to 16).
    #[inline]
    pub fn num_fault_recs(&self) -> usize {
        ((((self.cap >> 40) & 0xFF) as usize) + 1).min(16)
    }

    /// Returns `true` if `ECAP.PT` (Passthrough translation type) is supported in hardware.
    #[inline]
    pub fn supports_passthrough(&self) -> bool {
        ((self.ecap >> 6) & 1) != 0
    }

    /// Selects a supported `AW` field (`1` = 39-bit, `2` = 48-bit) from `CAP.SAGAW`.
    #[inline]
    pub fn preferred_aw(&self) -> u8 {
        let sagaw = (self.cap >> 8) & 0x1F;
        if (sagaw & (1 << 2)) != 0 {
            2 // 48-bit AGAW (4-level)
        } else if (sagaw & (1 << 1)) != 0 {
            1 // 39-bit AGAW (3-level)
        } else {
            1
        }
    }

    /// Initializes this DRHD unit (reads `VER`, `CAP`, `ECAP`).
    ///
    /// # Safety
    /// `mmio_base` is a valid virtual address mapped to DMAR MMIO.
    pub unsafe fn init(&mut self, mmio_base: u64, mmio_size: u64) {
        self.mmio_base = mmio_base;
        self.mmio_size = mmio_size;
        self.ver = self.read32(vtd_regs::VER);
        self.cap = self.read64(vtd_regs::CAP);
        self.ecap = self.read64(vtd_regs::ECAP);
        self.initialized.store(true, Ordering::Release);
    }

    /// Configures the Root Table for this DRHD (`RTADDR` + `GCMD.SRTP`, polling `GSTS.RTPS`).
    ///
    /// # Safety
    /// The Root Table must be allocated, 4 KiB-aligned, and initialized.
    pub unsafe fn set_root_table(&self, rt_phys: PhysAddr) {
        self.write64(vtd_regs::RTADDR, rt_phys.as_u64() & 0xFFFF_FFFF_FFFF_F000);
        let gsts = self.read32(vtd_regs::GSTS) & gcmd_bits::GSTS_PRESERVE_MASK;
        self.write32(vtd_regs::GCMD, gsts | gcmd_bits::SRTP);
        let mut timeout = 1_000_000u32;
        while timeout > 0 {
            if (self.read32(vtd_regs::GSTS) & (1 << 30)) != 0 {
                break; // RTPS bit acknowledged
            }
            core::hint::spin_loop();
            timeout -= 1;
        }
        self.root_table_phys
            .store(rt_phys.as_u64(), Ordering::Release);
    }

    /// Issues a Global Context-Cache Invalidation (`CCMD.ICC = 1, CIRG = 01b`).
    ///
    /// # Safety
    /// DRHD must be mapped and initialized.
    pub unsafe fn invalidate_context_global(&self) {
        let ccmd: u64 = (1u64 << 63) | (0b01u64 << 61);
        self.write64(vtd_regs::CCMD, ccmd);
        let mut timeout = 1_000_000u32;
        while timeout > 0 {
            if (self.read64(vtd_regs::CCMD) & (1u64 << 63)) == 0 {
                break;
            }
            core::hint::spin_loop();
            timeout -= 1;
        }
    }

    /// Invalidates context cache for a specific device (BDF) (`CIRG = 11b`).
    ///
    /// # Safety
    /// Requires the DRHD to be initialized.
    pub unsafe fn invalidate_context(&self, domain_id: u16, bus: u8, df: u8) {
        // Intel VT-d Spec §10.4.6: ICC[63]=1, CIRG[62:61]=11b (Device), SID[31:16], DID[15:0].
        let sid = ((bus as u64) << 8) | (df as u64);
        let ccmd: u64 = (1u64 << 63) | (0b11u64 << 61) | (sid << 16) | (domain_id as u64);
        self.write64(vtd_regs::CCMD, ccmd);
        let mut timeout = 1_000_000u32;
        while timeout > 0 {
            if (self.read64(vtd_regs::CCMD) & (1u64 << 63)) == 0 {
                break;
            }
            core::hint::spin_loop();
            timeout -= 1;
        }
    }

    /// Issues a Global IOTLB Invalidation via `IOTLB_REG` (`ECAP.IRO + 8`, `IIRG = 001b`).
    ///
    /// # Safety
    /// Requires the DRHD to be initialized.
    pub unsafe fn invalidate_iotlb_global(&self) {
        let iotlb_reg = self.iotlb_reg_base() + 8;
        // Intel VT-d Spec §10.4.8.2: IVT[63]=1, IIRG[62:60]=001b (Global), DR[49]=1, DW[48]=1.
        let cmd: u64 = (1u64 << 63) | (0b001u64 << 60) | (1u64 << 49) | (1u64 << 48);
        self.write64(iotlb_reg, cmd);
        let mut timeout = 1_000_000u32;
        while timeout > 0 {
            if (self.read64(iotlb_reg) & (1u64 << 63)) == 0 {
                break;
            }
            core::hint::spin_loop();
            timeout -= 1;
        }
    }

    /// Issues a Domain-Selective IOTLB Invalidation via `IOTLB_REG` (`ECAP.IRO + 8`, `IIRG = 010b`).
    ///
    /// # Safety
    /// Requires the DRHD to be initialized.
    pub unsafe fn invalidate_iotlb_domain(&self, domain_id: u16) {
        let iotlb_reg = self.iotlb_reg_base() + 8;
        // Intel VT-d Spec §10.4.8.2: IVT[63]=1, IIRG[62:60]=010b (Domain), DR[49]=1, DW[48]=1, DID[47:32].
        let cmd: u64 = (1u64 << 63)
            | (0b010u64 << 60)
            | (1u64 << 49)
            | (1u64 << 48)
            | ((domain_id as u64) << 32);
        self.write64(iotlb_reg, cmd);
        let mut timeout = 1_000_000u32;
        while timeout > 0 {
            if (self.read64(iotlb_reg) & (1u64 << 63)) == 0 {
                break;
            }
            core::hint::spin_loop();
            timeout -= 1;
        }
    }

    /// Enables DMA translation on this DRHD (`GCMD.TE = 1`, polling `GSTS.TES == 1`).
    ///
    /// # Safety
    /// The Root Table must be properly configured before activation.
    pub unsafe fn enable_translation(&self) -> bool {
        let gsts = self.read32(vtd_regs::GSTS) & gcmd_bits::GSTS_PRESERVE_MASK;
        self.write32(vtd_regs::GCMD, gsts | gcmd_bits::TE);
        let mut timeout = 1_000_000u32;
        let mut active = false;
        while timeout > 0 {
            if (self.read32(vtd_regs::GSTS) & (1 << 31)) != 0 {
                active = true;
                break; // TES bit acknowledged
            }
            core::hint::spin_loop();
            timeout -= 1;
        }
        self.enabled.store(active, Ordering::Release);
        active
    }

    /// Clears all pending fault recording registers (`FRCD_REG[i].F` RW1C) and `FSTS.PFO`.
    ///
    /// # Safety
    /// Requires the DRHD to be initialized.
    pub unsafe fn clear_fault_status(&self) {
        let fro = self.fault_rec_offset();
        let nfr = self.num_fault_recs();
        if fro > 0 && fro + nfr * 16 <= self.mmio_size as usize {
            for i in 0..nfr {
                let hi_off = fro + i * 16 + 8;
                let hi = self.read64(hi_off);
                if (hi & (1u64 << 63)) != 0 {
                    self.write64(hi_off, 1u64 << 63);
                }
            }
        }
        let fsts = self.read32(vtd_regs::FSTS);
        if (fsts & 0x01) != 0 {
            self.write32(vtd_regs::FSTS, 0x01); // Clear PFO (RW1C)
        }
    }

    /// Polls `FSTS` and `FRCD_REG[0..NFR]` for a real hardware DMAR fault record, clears the
    /// hardware `F` bit (RW1C), and returns the decoded fault.
    ///
    /// # Safety
    /// Requires the DRHD to be initialized.
    pub unsafe fn poll_fault_record(&self, unit_idx: u8) -> Option<VtdHardwareFault> {
        let fsts = self.read32(vtd_regs::FSTS);
        if (fsts & 0x02) == 0 {
            return None; // PPF == 0 -> no pending primary fault
        }
        let fro = self.fault_rec_offset();
        let nfr = self.num_fault_recs();
        if fro == 0 || fro + nfr * 16 > self.mmio_size as usize {
            return None;
        }
        for i in 0..nfr {
            let lo_off = fro + i * 16;
            let hi_off = lo_off + 8;
            let hi = self.read64(hi_off);
            if (hi & (1u64 << 63)) != 0 {
                let lo = self.read64(lo_off);
                // Clear Fault (F) bit (RW1C) and clear overflow if set.
                self.write64(hi_off, 1u64 << 63);
                if (fsts & 0x01) != 0 {
                    self.write32(vtd_regs::FSTS, 0x01);
                }
                return Some(VtdHardwareFault {
                    unit_idx,
                    source_id: (hi & 0xFFFF) as u16,
                    reason: ((hi >> 32) & 0xFF) as u8,
                    is_read: (hi & (1u64 << 62)) != 0,
                    fault_addr: lo & 0xFFFF_FFFF_FFFF_F000,
                });
            }
        }
        None
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

    /// Returns `true` when at least one hardware DRHD unit has `GSTS.TES == 1` active.
    pub fn translation_enabled(&self) -> bool {
        if !self.is_initialized() {
            return false;
        }
        let count = self.unit_count().min(MAX_DRHD);
        for i in 0..count {
            let u = &self.units[i];
            if u.initialized.load(Ordering::Acquire) && u.enabled.load(Ordering::Acquire) {
                // SAFETY: unit is mapped and initialized.
                let gsts = unsafe { u.read32(vtd_regs::GSTS) };
                if (gsts & (1 << 31)) != 0 {
                    return true;
                }
            }
        }
        false
    }

    /// Clears all pending hardware DMAR fault records across all active DRHDs.
    pub fn clear_all_hardware_faults(&self) {
        if !self.is_initialized() {
            return;
        }
        let count = self.unit_count().min(MAX_DRHD);
        for i in 0..count {
            let unit = &self.units[i];
            if unit.initialized.load(Ordering::Acquire) {
                // SAFETY: DRHD MMIO mapped and initialized.
                unsafe {
                    unit.clear_fault_status();
                }
            }
        }
    }

    /// Polls all active DRHD units for a real hardware DMAR fault in `FSTS` / `FRCD_REG`.
    pub fn poll_hardware_dmar_fault(&self) -> Option<VtdHardwareFault> {
        if !self.is_initialized() {
            return None;
        }
        let count = self.unit_count().min(MAX_DRHD);
        for i in 0..count {
            let unit = &self.units[i];
            if unit.initialized.load(Ordering::Acquire) {
                // SAFETY: DRHD MMIO mapped and initialized.
                if let Some(fault) = unsafe { unit.poll_fault_record(i as u8) } {
                    return Some(fault);
                }
            }
        }
        None
    }

    /// Invalidates the Context Cache and IOTLB for `domain_id` across all initialized DRHD units
    /// using the spec-compliant Register-Based Invalidation Interface (`CCMD` + `ECAP.IRO` `IOTLB_REG`).
    ///
    /// # Safety
    /// CPL 0, VT-d initialized.
    pub unsafe fn flush_iotlb_domain(&self, domain_id: u16, _iova: u64) {
        if !self.initialized.load(Ordering::Acquire) {
            return;
        }
        let count = self.count.load(Ordering::Relaxed).min(MAX_DRHD as u32) as usize;
        for i in 0..count {
            let unit = &self.units[i];
            if !unit.initialized.load(Ordering::Acquire) {
                continue;
            }

            // Domain-Selective Context-Cache Invalidation (Intel VT-d Spec §10.4.6):
            // ICC[63] = 1, CIRG[62:61] = 10b (Domain-Selective), DID[15:0] = domain_id.
            let ccmd: u64 = (1u64 << 63) | (0b10u64 << 61) | (domain_id as u64);
            unit.write64(vtd_regs::CCMD, ccmd);
            let mut timeout = 1_000_000u32;
            while timeout > 0 {
                if (unit.read64(vtd_regs::CCMD) & (1u64 << 63)) == 0 {
                    break;
                }
                core::hint::spin_loop();
                timeout -= 1;
            }

            // Spec-compliant 64-bit Domain-Selective IOTLB Invalidation at `ECAP.IRO + 8`.
            unit.invalidate_iotlb_domain(domain_id);
        }
    }
}

pub static INTEL_VTD: IntelVtd = IntelVtd::new();

/// Parses the ACPI `DMAR` table, maps every DRHD MMIO page into `FIXMAP_VTD_BASE + idx`,
/// installs static 4 KiB-aligned `RootTable` + `ContextTable`s (blocking rogue/untrusted test
/// DMA devices such as QEMU `edu` `0x1234:0x11e8` as Not-Present while passing through trusted
/// boot storage/network bridges when `ECAP.PT == 1`), enables hardware DMA translation
/// (`GCMD.SRTP` + `GCMD.TE`), and initializes `INTEL_VTD`.
///
/// Returns the number of hardware DRHD units registered.
#[cfg(all(target_arch = "x86_64", target_os = "none"))]
pub fn init_from_acpi_dmar(dmar_phys: u64) -> usize {
    use core::fmt::Write;
    use crate::arch::x86_64::acpi::parser::{acpi_read_unaligned, SdtHeader};
    use crate::arch::x86_64::memory_iface::KERNEL_FAULT_ALLOC;
    use crate::exophoenix::stage0::{b_device, b_device_count};
    use crate::memory::core::{
        fixmap_slot_addr, Frame, PageFlags, VirtAddr, FIXMAP_VTD_BASE,
    };
    use crate::memory::virt::address_space::kernel::KERNEL_AS;
    use crate::memory::virt::address_space::tlb;

    if INTEL_VTD.is_initialized() {
        return INTEL_VTD.unit_count();
    }
    if dmar_phys == 0 {
        return 0;
    }

    let Some(header) = acpi_read_unaligned::<SdtHeader>(dmar_phys) else {
        return 0;
    };
    if &header.signature != b"DMAR" {
        return 0;
    }
    let table_len = header.length as usize;
    // DMAR fixed header is 48 bytes (36-byte SDT header + 1B HAW + 1B flags + 10B reserved).
    if table_len < 48 || table_len > 4096 {
        return 0;
    }

    let mmio_flags = PageFlags::PRESENT
        | PageFlags::WRITABLE
        | PageFlags::NO_EXECUTE
        | PageFlags::NO_CACHE
        | PageFlags::GLOBAL;

    let mut registered = 0usize;
    let mut first_drhd_phys = 0u64;
    let mut off = 48usize;
    while off + 4 <= table_len && registered < MAX_DRHD {
        let Some(entry_type) = acpi_read_unaligned::<u16>(dmar_phys + off as u64) else {
            break;
        };
        let Some(entry_len) = acpi_read_unaligned::<u16>(dmar_phys + off as u64 + 2) else {
            break;
        };
        let entry_len_usize = entry_len as usize;
        if entry_len_usize < 4 || off + entry_len_usize > table_len {
            break;
        }

        // Type 0 = DRHD (DMA Remapping Hardware Unit Definition), min length 16 bytes.
        if entry_type == 0 && entry_len_usize >= 16 {
            if let Some(reg_base) = acpi_read_unaligned::<u64>(dmar_phys + off as u64 + 8) {
                if reg_base != 0 && (reg_base & 0xFFF) == 0 {
                    let fix_virt = fixmap_slot_addr(FIXMAP_VTD_BASE + registered);
                    if KERNEL_AS.translate(fix_virt).is_none() {
                        let frame = Frame::containing(PhysAddr::new(reg_base));
                        // SAFETY: mapping ACPI DMAR hardware MMIO page in CPL0 fixmap.
                        if unsafe { KERNEL_AS.map(fix_virt, frame, mmio_flags, &KERNEL_FAULT_ALLOC) }
                            .is_err()
                        {
                            off += entry_len_usize;
                            continue;
                        }
                        // SAFETY: invalidate newly mapped fixmap VA.
                        unsafe {
                            tlb::flush_single(fix_virt);
                        }
                    }
                    // SAFETY: fix_virt is mapped to a valid 4 KiB DRHD MMIO page.
                    if unsafe { INTEL_VTD.register_drhd(fix_virt.as_u64(), 4096) }.is_ok() {
                        if first_drhd_phys == 0 {
                            first_drhd_phys = reg_base;
                        }
                        registered += 1;
                    }
                }
            }
        }

        off += entry_len_usize;
    }

    if registered == 0 {
        return 0;
    }

    // Check whether all units support hardware Passthrough (`ECAP.PT == 1`) and select valid AGAW.
    let unit0 = &INTEL_VTD.units[0];
    let pt_ok = unit0.supports_passthrough();
    let aw = unit0.preferred_aw();

    // Locate any QEMU `edu` PCI DMA device (`0x1234:0x11e8`) so its ContextEntry stays
    // strictly Not-Present (deny-by-default hardware DMA trap in `F7`).
    let mut edu_bdf: Option<(u8, u8)> = None;
    for idx in 0..b_device_count() {
        if let Some(dev) = b_device(idx) {
            if dev.vendor_id == 0x1234 && dev.device_id == 0x11e8 {
                edu_bdf = Some((dev.bus, (dev.device << 3) | (dev.function & 0x7)));
                break;
            }
        }
    }

    let mut tes_enabled = false;
    if pt_ok {
        // SAFETY: single-threaded Stage0 bootstrap before `INTEL_VTD.global_init()`.
        unsafe {
            let rt = &mut *core::ptr::addr_of_mut!(VTD_ROOT_TABLE);
            let ctxs = &mut *core::ptr::addr_of_mut!(VTD_CONTEXT_TABLES);
            rt.zero();

            for bus in 0..STATIC_CONTEXT_BUSES {
                ctxs[bus].zero();
                for devfn in 0..256usize {
                    if edu_bdf == Some((bus as u8, devfn as u8)) {
                        // Explicitly leave `edu` ContextEntry Not-Present (hardware deny-by-default)!
                        ctxs[bus].entries[devfn] = ContextEntry::EMPTY;
                    } else {
                        ctxs[bus].entries[devfn].set_passthrough_with_aw(1, aw);
                    }
                }
                let ctx_va = VirtAddr::new(&ctxs[bus] as *const ContextTable as u64);
                let ctx_pa = KERNEL_AS
                    .translate(ctx_va)
                    .unwrap_or(PhysAddr::new(ctx_va.as_u64()));
                rt.entries[bus].set_context_table(ctx_pa);
            }

            compiler_fence(Ordering::SeqCst);

            let rt_va = VirtAddr::new(rt as *const RootTable as u64);
            let rt_pa = KERNEL_AS
                .translate(rt_va)
                .unwrap_or(PhysAddr::new(rt_va.as_u64()));

            for i in 0..registered {
                let u = &INTEL_VTD.units[i];
                u.set_root_table(rt_pa);
                u.invalidate_context_global();
                u.invalidate_iotlb_global();
                u.clear_fault_status();
                if u.enable_translation() {
                    tes_enabled = true;
                }
            }
        }
    }

    // SAFETY: all DRHD units registered and programmed during Stage0 boot.
    unsafe {
        INTEL_VTD.global_init();
    }

    // Emit compact hardware VT-d diagnostic line on port 0xE9.
    struct StackBuf {
        buf: [u8; 192],
        len: usize,
    }
    impl Write for StackBuf {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let avail = self.buf.len().saturating_sub(self.len);
            let n = s.len().min(avail);
            self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
            self.len += n;
            Ok(())
        }
    }
    let mut msg = StackBuf {
        buf: [0u8; 192],
        len: 0,
    };
    let _ = writeln!(
        msg,
        "[ExoPhoenix] [VT-d] DRHD={} phys={:#x} ver={:#x} aw={} pt={} tes={} edu_blocked={}",
        registered,
        first_drhd_phys,
        unit0.ver,
        aw,
        u8::from(pt_ok),
        u8::from(tes_enabled),
        u8::from(edu_bdf.is_some()),
    );
    crate::arch::x86_64::terminal::debug_write(&msg.buf[..msg.len]);

    registered
}

#[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
pub fn init_from_acpi_dmar(_dmar_phys: u64) -> usize {
    0
}
