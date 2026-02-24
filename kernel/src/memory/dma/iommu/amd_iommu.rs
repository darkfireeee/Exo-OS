// kernel/src/memory/dma/iommu/amd_iommu.rs
//
// Pilote AMD IOMMU (IOMMU Technology for AMD64).
// Référence : AMD IOMMU Architecture Specification, Rev 3.05.
// COUCHE 0 — aucune dépendance externe.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use crate::memory::core::types::PhysAddr;
use crate::memory::dma::core::types::DmaError;

// ─────────────────────────────────────────────────────────────────────────────
// OFFSETS MMIO AMD IOMMU
// ─────────────────────────────────────────────────────────────────────────────

pub mod amd_iommu_regs {
    pub const DEV_TABLE_BA:   usize = 0x000; // Device Table Base Address register
    pub const CMD_BUF_BA:     usize = 0x008; // Command Buffer Base Address
    pub const EVT_LOG_BA:     usize = 0x010; // Event Log Base Address
    pub const CTRL:           usize = 0x018; // Control Register (64-bit)
    pub const EXCL_BASE:      usize = 0x020; // Exclusion Range Base
    pub const EXCL_LIMIT:     usize = 0x028; // Exclusion Range Limit
    pub const EXT_FEAT:       usize = 0x030; // Extended Feature Register
    pub const CMD_BUF_HEAD:   usize = 0x2000; // Command Buffer Head Pointer
    pub const CMD_BUF_TAIL:   usize = 0x2008; // Command Buffer Tail Pointer
    pub const EVT_LOG_HEAD:   usize = 0x2010; // Event Log Head Pointer
    pub const EVT_LOG_TAIL:   usize = 0x2018; // Event Log Tail Pointer
    pub const STATUS:         usize = 0x2020; // Status Register
    pub const PPR_LOG_HEAD:   usize = 0x2030; // PPR Log Head
    pub const PPR_LOG_TAIL:   usize = 0x2038; // PPR Log Tail
}

/// Bits du registre de contrôle AMD IOMMU.
pub mod ctrl_bits {
    pub const IOMMU_EN:        u64 = 1 << 0;  // IOMMU Enable
    pub const HT_TUN_EN:       u64 = 1 << 1;  // HyperTransport Tunnel Enable
    pub const EVT_LOG_EN:      u64 = 1 << 2;  // Event Log Enable
    pub const EVT_INT_EN:      u64 = 1 << 3;  // Event Log Interrupt Enable
    pub const COMP_WAIT_INT:   u64 = 1 << 4;  // Completion Wait Interrupt Enable
    pub const INV_TIMEOUT:     u64 = 0x7 << 5; // Invalidation Timeout
    pub const PASS_PW:         u64 = 1 << 8;  // Pass Posted Write
    pub const RESP_PASS_PW:    u64 = 1 << 9;  // Response Pass Posted Write
    pub const COHERENT:        u64 = 1 << 10; // Coherent access
    pub const ISOC:            u64 = 1 << 11; // Isochronous access
    pub const CMD_BUF_EN:      u64 = 1 << 12; // Command Buffer Enable
    pub const PPR_LOG_EN:      u64 = 1 << 13; // PPR Log Enable
    pub const PPR_INT_EN:      u64 = 1 << 14; // PPR Interrupt Enable
    pub const PPR_EN:          u64 = 1 << 15; // PPR Enable
    pub const GT_EN:           u64 = 1 << 16; // Guest Translation Enable
    pub const GA_EN:           u64 = 1 << 17; // Guest APIC Mode Enable
    pub const SMIF_EN:         u64 = 1 << 22; // SMI Filter Enable
    pub const SMIF_LOG_EN:     u64 = 1 << 24; // SMI Filter Log Enable
    pub const GAM_EN:          u64 = 1 << 25; // GAPIC Enable
    pub const DUAL_PPR_LOG_EN: u64 = 1 << 28; // Dual PPR Log Enable
    pub const DUAL_EVT_LOG_EN: u64 = 1 << 30; // Dual Event Log Enable
    pub const DEVTBL_SEG_EN:   u64 = 1 << 34; // Device Table Segmentation Enable
    pub const PRIVABRT_EN:     u64 = 1 << 59; // PrivAbort Enable
}

// ─────────────────────────────────────────────────────────────────────────────
// DEVICE TABLE ENTRY (DTE) AMD
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée DTE (Device Table Entry) AMD — 32 octets par device.
#[repr(C, align(32))]
#[derive(Copy, Clone)]
pub struct AmdDte {
    pub dw0: u64,
    pub dw1: u64,
    pub dw2: u64,
    pub dw3: u64,
}

impl AmdDte {
    pub const EMPTY: Self = AmdDte { dw0: 0, dw1: 0, dw2: 0, dw3: 0 };

    // Bits DW0.
    pub const V:    u64 = 1 << 0;   // Valid
    pub const TV:   u64 = 1 << 1;   // Translation Valid
    // DW0[51:12] = page table root physical address.
    pub const PT_MASK: u64 = 0x000F_FFFF_FFFF_F000;

    // DW1[16:0] = DomainID.
    pub const DOM_MASK: u64 = 0xFFFF;

    #[inline]
    pub fn is_valid(self) -> bool { self.dw0 & Self::V != 0 }

    /// Configure la DTE pour un domaine traduit.
    ///
    /// `pt_phys` — adresse physique de la table de pages IOMMU (niveau supérieur).
    /// `domain_id` — identifiant du domaine (16-bit).
    /// `mode` — niveau de table: 0=passthrough, 1..6=niveaux.
    pub fn configure_translated(&mut self, pt_phys: PhysAddr, domain_id: u16, mode: u8) {
        // DW0: V=1, TV=1, PTE Root, Mode (bits 61:59).
        self.dw0 = Self::V | Self::TV
                 | (pt_phys.as_u64() & Self::PT_MASK)
                 | ((mode as u64 & 0x7) << 9);
        // DW1: DomainID.
        self.dw1 = domain_id as u64;
        self.dw2 = 0;
        self.dw3 = 0;
    }

    pub fn configure_passthrough(&mut self) {
        self.dw0 = Self::V | 0x2; // HostTranslate
        self.dw1 = 0;
        self.dw2 = 0;
        self.dw3 = 0;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// COMMANDE AMD IOMMU
// ─────────────────────────────────────────────────────────────────────────────

/// Une commande AMD IOMMU (16 octets).
#[repr(C, align(16))]
#[derive(Copy, Clone)]
pub struct AmdIommuCmd {
    pub dw0: u64,
    pub dw1: u64,
}

impl AmdIommuCmd {
    pub const OPCODE_COMPLETION_WAIT:   u64 = 0x01;
    pub const OPCODE_INVAL_DEVTAB_ENTRY: u64 = 0x02;
    pub const OPCODE_INVAL_IOMMU_PAGES: u64 = 0x03;
    pub const OPCODE_INVAL_IOTLB_PAGES: u64 = 0x04;
    pub const OPCODE_INVAL_INTR_TABLE:  u64 = 0x05;
    pub const OPCODE_PREFETCH_IOMMU_PAGES: u64 = 0x06;
    pub const OPCODE_COMPLETE_PPR_REQ:  u64 = 0x07;

    /// Commande : Invalidation de l'entrée Device Table (BDF, pasid=0).
    pub fn invalidate_devtab(bdf: u16) -> Self {
        AmdIommuCmd {
            dw0: (Self::OPCODE_INVAL_DEVTAB_ENTRY << 60)
               | (bdf as u64 & 0xFFFF),
            dw1: 0,
        }
    }

    /// Commande : Invalidation des pages IOMMU (domaine entier).
    pub fn invalidate_iommu_all(domain_id: u16) -> Self {
        AmdIommuCmd {
            dw0: (Self::OPCODE_INVAL_IOMMU_PAGES << 60)
               | ((domain_id as u64) << 32)
               | (1 << 0),  // S (Size) = all pages
            dw1: 0x7FFFFFFF_FFFFF000 | 1, // Addr=0, S=1
        }
    }

    /// Commande : Completion Wait (synchronisation).
    pub fn completion_wait(store_addr: u64, data: u64) -> Self {
        AmdIommuCmd {
            dw0: (Self::OPCODE_COMPLETION_WAIT << 60)
               | (store_addr & !7u64)
               | (1 << 0), // S=1 (store)
            dw1: data,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CONTRÔLEUR AMD IOMMU
// ─────────────────────────────────────────────────────────────────────────────

pub const MAX_AMD_IOMMU: usize = 4;
/// Taille de la Device Table (65536 entrées × 32 octets = 2 MiB).
pub const DEVTAB_SIZE: usize = 65536;
/// Capacité du Command Buffer (4096 entrées × 16 octets = 64 KiB).
pub const CMD_BUF_CAPACITY: usize = 4096;

pub struct AmdIommuUnit {
    pub mmio_base:  u64,
    pub ext_feat:   u64,
    pub devtab_phys: AtomicU64,
    pub cmdbuf_phys: AtomicU64,
    pub enabled:    AtomicBool,
}

impl AmdIommuUnit {
    const fn new_uninit() -> Self {
        AmdIommuUnit {
            mmio_base:   0,
            ext_feat:    0,
            devtab_phys: AtomicU64::new(0),
            cmdbuf_phys: AtomicU64::new(0),
            enabled:     AtomicBool::new(false),
        }
    }

    unsafe fn read64(&self, offset: usize) -> u64 {
        let ptr = (self.mmio_base + offset as u64) as *const u64;
        core::ptr::read_volatile(ptr)
    }

    unsafe fn write64(&self, offset: usize, val: u64) {
        let ptr = (self.mmio_base + offset as u64) as *mut u64;
        core::ptr::write_volatile(ptr, val);
    }

    /// Initialise l'unité AMD IOMMU depuis son adresse MMIO.
    ///
    /// # Safety
    /// `mmio_base` est l'adresse virtuelle mappée du registre AMD IOMMU.
    pub unsafe fn init(&mut self, mmio_base: u64) {
        self.mmio_base = mmio_base;
        self.ext_feat = self.read64(amd_iommu_regs::EXT_FEAT);
    }

    /// Configure la Device Table.
    ///
    /// # Safety
    /// `devtab_phys` doit pointer un tableau de 65536 DTEs zéro-initialisé.
    pub unsafe fn set_device_table(&self, devtab_phys: PhysAddr) {
        // DTB size encoding: 0b111 = 65536 entrées.
        let dtsz = 0b111u64;
        self.write64(amd_iommu_regs::DEV_TABLE_BA, devtab_phys.as_u64() | dtsz);
        self.devtab_phys.store(devtab_phys.as_u64(), Ordering::Release);
    }

    /// Configure et active le Command Buffer.
    ///
    /// # Safety
    /// `cmdbuf_phys` doit pointer un buffer de CMD_BUF_CAPACITY × 16 octets.
    pub unsafe fn enable_cmdbuf(&self, cmdbuf_phys: PhysAddr) {
        // Size encoding pour 4096 entrées: 0b1100 (2^12 = 4096).
        let len_encoding = 12u64;
        let ba = cmdbuf_phys.as_u64() | (len_encoding << 56);
        self.write64(amd_iommu_regs::CMD_BUF_BA, ba);
        // Head/Tail à zéro.
        self.write64(amd_iommu_regs::CMD_BUF_HEAD, 0);
        self.write64(amd_iommu_regs::CMD_BUF_TAIL, 0);
        self.cmdbuf_phys.store(cmdbuf_phys.as_u64(), Ordering::Release);
        // Active le buffer.
        let ctrl = self.read64(amd_iommu_regs::CTRL);
        self.write64(amd_iommu_regs::CTRL, ctrl | ctrl_bits::CMD_BUF_EN);
    }

    /// Active l'IOMMU.
    ///
    /// # Safety
    /// Device Table et Command Buffer doivent être configurés.
    pub unsafe fn enable(&self) {
        let ctrl = self.read64(amd_iommu_regs::CTRL);
        self.write64(amd_iommu_regs::CTRL, ctrl | ctrl_bits::IOMMU_EN);
        self.enabled.store(true, Ordering::Release);
    }

    pub fn is_enabled(&self) -> bool { self.enabled.load(Ordering::Acquire) }
}

pub struct AmdIommuController {
    units:       [AmdIommuUnit; MAX_AMD_IOMMU],
    unit_count:  AtomicU32,
    initialized: AtomicBool,
}

unsafe impl Sync for AmdIommuController {}
unsafe impl Send for AmdIommuController {}

impl AmdIommuController {
    const fn new() -> Self {
        AmdIommuController {
            units: [
                AmdIommuUnit::new_uninit(), AmdIommuUnit::new_uninit(),
                AmdIommuUnit::new_uninit(), AmdIommuUnit::new_uninit(),
            ],
            unit_count:  AtomicU32::new(0),
            initialized: AtomicBool::new(false),
        }
    }

    /// Enregistre une unit AMD IOMMU depuis l'IVRS ACPI.
    ///
    /// # Safety
    /// `mmio_base` est l'adresse MMIO mappée.
    pub unsafe fn register_unit(&self, mmio_base: u64) -> Result<(), DmaError> {
        let idx = self.unit_count.load(Ordering::Relaxed) as usize;
        if idx >= MAX_AMD_IOMMU { return Err(DmaError::OutOfMemory); }
        let ptr = &self.units[idx] as *const AmdIommuUnit as *mut AmdIommuUnit;
        (*ptr).init(mmio_base);
        self.unit_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub fn unit_count(&self) -> usize { self.unit_count.load(Ordering::Relaxed) as usize }
    pub fn is_initialized(&self) -> bool { self.initialized.load(Ordering::Acquire) }

    pub fn mark_initialized(&self) { self.initialized.store(true, Ordering::Release); }
}

pub static AMD_IOMMU: AmdIommuController = AmdIommuController::new();
