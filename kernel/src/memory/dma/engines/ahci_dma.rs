// kernel/src/memory/dma/engines/ahci_dma.rs
//
// Moteur DMA SATA AHCI (Advanced Host Controller Interface).
// Ref : AHCI 1.3.1 Specification.
// Couche 0 — no_std, accès MMIO uniquement via raw pointers.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

use crate::memory::core::PhysAddr;
use crate::memory::dma::stats::counters::{
    dma_stat_complete, dma_stat_error, dma_stat_submit, DMA_STATS,
};

// ─────────────────────────────────────────────────────────────────────────────
// REGISTRES AHCI HBA
// ─────────────────────────────────────────────────────────────────────────────

#[allow(dead_code)]
mod hba_regs {
    /// Host Capabilities.
    pub const CAP: usize = 0x00;
    /// Global Host Control.
    pub const GHC: usize = 0x04;
    /// Interrupt Status.
    pub const IS: usize = 0x08;
    /// Ports Implemented.
    pub const PI: usize = 0x0C;
    /// Version.
    pub const VS: usize = 0x10;

    /// Base de chaque port (0x100 + port * 0x80).
    pub const PORT_BASE: usize = 0x100;
    pub const PORT_STRIDE: usize = 0x80;
}

#[allow(dead_code)]
mod port_regs {
    /// Command List Base Address (low 32 bits).
    pub const CLB: usize = 0x00;
    /// Command List Base Address (high 32 bits).
    pub const CLBU: usize = 0x04;
    /// FIS Base Address (low 32 bits).
    pub const FB: usize = 0x08;
    /// FIS Base Address (high 32 bits).
    pub const FBU: usize = 0x0C;
    /// Interrupt Status.
    pub const IS: usize = 0x10;
    /// Interrupt Enable.
    pub const IE: usize = 0x14;
    /// Command and Status.
    pub const CMD: usize = 0x18;
    /// Task File Data.
    pub const TFD: usize = 0x20;
    /// Signature.
    pub const SIG: usize = 0x24;
    /// Serial ATA Status (SCR0 : SStatus).
    pub const SSTS: usize = 0x28;
    /// Serial ATA Control (SCR2 : SControl).
    pub const SCTL: usize = 0x2C;
    /// Serial ATA Error (SCR1 : SError).
    pub const SERR: usize = 0x30;
    /// Serial ATA Active.
    pub const SACT: usize = 0x34;
    /// Command Issue.
    pub const CI: usize = 0x38;
}

// ─────────────────────────────────────────────────────────────────────────────
// STRUCTURES AHCI
// ─────────────────────────────────────────────────────────────────────────────

/// PRDT Entry (Physical Region Descriptor Table).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct AhciPrdtEntry {
    /// Data Base Address (low).
    pub dba_lo: u32,
    /// Data Base Address (high).
    pub dba_hi: u32,
    pub _reserved: u32,
    /// Data Byte Count (bits 21:0), bit 31 = interrupt on completion.
    pub dbc: u32,
}

impl AhciPrdtEntry {
    pub fn new(phys: PhysAddr, byte_count: u32, irq_on_cmp: bool) -> Self {
        AhciPrdtEntry {
            dba_lo: (phys.as_u64() & 0xFFFF_FFFF) as u32,
            dba_hi: (phys.as_u64() >> 32) as u32,
            _reserved: 0,
            dbc: (byte_count - 1) | (if irq_on_cmp { 1 << 31 } else { 0 }),
        }
    }
}

/// Command FIS (RegH2D, 20 bytes, padded to 64 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AhciCommandFis {
    /// FIS type : 0x27 = Register H2D.
    pub fis_type: u8,
    /// Bit 7 = Command, bits 3:0 = Port Multiplier Port.
    pub flags: u8,
    /// Commande ATA.
    pub command: u8,
    /// Features (low).
    pub featurel: u8,
    /// LBA 7:0.
    pub lba0: u8,
    /// LBA 15:8.
    pub lba1: u8,
    /// LBA 23:16.
    pub lba2: u8,
    /// Device register (LBA48 : bit 6).
    pub device: u8,
    /// LBA 31:24.
    pub lba3: u8,
    /// LBA 39:32.
    pub lba4: u8,
    /// LBA 47:40.
    pub lba5: u8,
    /// Features (high).
    pub featureh: u8,
    /// Sector count (low).
    pub countl: u8,
    /// Sector count (high).
    pub counth: u8,
    /// Reserved (isochronous).
    pub icc: u8,
    /// Control register.
    pub control: u8,
    pub _aux: [u8; 48],
}

impl AhciCommandFis {
    const FIS_TYPE_REG_H2D: u8 = 0x27;
    const CMD_FLAG: u8 = 1 << 7;
    const ATA_CMD_READ_DMA_EXT: u8 = 0x25;
    const ATA_CMD_WRITE_DMA_EXT: u8 = 0x35;

    pub fn new_read_dma48(lba: u64, sector_count: u16) -> Self {
        let mut f = Self::default();
        f.fis_type = Self::FIS_TYPE_REG_H2D;
        f.flags = Self::CMD_FLAG;
        f.command = Self::ATA_CMD_READ_DMA_EXT;
        f.device = 1 << 6; // LBA mode
        f.lba0 = (lba & 0xFF) as u8;
        f.lba1 = ((lba >> 8) & 0xFF) as u8;
        f.lba2 = ((lba >> 16) & 0xFF) as u8;
        f.lba3 = ((lba >> 24) & 0xFF) as u8;
        f.lba4 = ((lba >> 32) & 0xFF) as u8;
        f.lba5 = ((lba >> 40) & 0xFF) as u8;
        f.countl = (sector_count & 0xFF) as u8;
        f.counth = (sector_count >> 8) as u8;
        f
    }

    pub fn new_write_dma48(lba: u64, sector_count: u16) -> Self {
        let mut f = Self::new_read_dma48(lba, sector_count);
        f.command = Self::ATA_CMD_WRITE_DMA_EXT;
        f
    }

    fn default() -> Self {
        // SAFETY: repr(C), tous les champs sont des entiers/tableaux, 0 est valide.
        unsafe { core::mem::zeroed() }
    }
}

/// Command Header (32 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct AhciCommandHeader {
    /// Bits 15:0 = PRDTL (nb PRDT entries).
    /// Bit 6 = Write, bit 5 = Atapi, bit 2 = Reset, bit 0 = FIS Length/4.
    pub dw0: u32,
    /// PRD Byte Count (cleared par HW).
    pub prdbc: u32,
    /// Command Table Base Address (low, 128-byte aligné).
    pub ctba_lo: u32,
    /// Command Table Base Address (high).
    pub ctba_hi: u32,
    pub _rsvd: [u32; 4],
}

impl AhciCommandHeader {
    /// `write` = true pour Write DMA, false pour Read DMA.
    pub fn new(fis_len_dw: u8, prdtl: u16, ctba: PhysAddr, write: bool) -> Self {
        let mut dw0 = (fis_len_dw & 0x1F) as u32; // FIS length en DW
        dw0 |= (prdtl as u32) << 16;
        if write {
            dw0 |= 1 << 6;
        }
        AhciCommandHeader {
            dw0,
            prdbc: 0,
            ctba_lo: (ctba.as_u64() & 0xFFFF_FFFF) as u32,
            ctba_hi: (ctba.as_u64() >> 32) as u32,
            _rsvd: [0; 4],
        }
    }
}

/// Taille max du command table interne (1 PRDT entry).
const MAX_PRDT: usize = 8;

/// Command Table (FIS + PRDT).
#[repr(C, align(128))]
#[derive(Clone, Copy)]
struct AhciCmdTable {
    cfis: AhciCommandFis,
    acmd: [u8; 16], // ATAPI command (non utilisé pour DMA)
    _rsvd: [u8; 48],
    prdt: [AhciPrdtEntry; MAX_PRDT],
}

// ─────────────────────────────────────────────────────────────────────────────
// SLOT INTERNE
// ─────────────────────────────────────────────────────────────────────────────

const AHCI_SLOTS: usize = 32;

#[derive(Clone, Copy)]
struct AhciSlotStatus {
    occupied: bool,
    write_op: bool,
    size_bytes: u32,
}

impl Default for AhciSlotStatus {
    fn default() -> Self {
        AhciSlotStatus {
            occupied: false,
            write_op: false,
            size_bytes: 0,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MOTEUR AHCI DMA
// ─────────────────────────────────────────────────────────────────────────────

struct AhciState {
    cmd_headers: [AhciCommandHeader; AHCI_SLOTS],
    cmd_tables: [AhciCmdTable; AHCI_SLOTS],
    slots: [AhciSlotStatus; AHCI_SLOTS],
}

impl AhciState {
    #[allow(dead_code)]
    fn new() -> Self {
        // SAFETY: tous les champs représentés avec 0.
        unsafe {
            AhciState {
                cmd_headers: core::mem::zeroed(),
                cmd_tables: core::mem::zeroed(),
                slots: [AhciSlotStatus::default(); AHCI_SLOTS],
            }
        }
    }
}

/// Moteur AHCI DMA.
pub struct AhciDmaEngine {
    hba_base: AtomicU64,
    port_idx: AtomicU32,
    present: AtomicBool,
    engine_id: AtomicU32,
    state: Mutex<AhciState>,
}

impl AhciDmaEngine {
    const fn new_uninit() -> Self {
        AhciDmaEngine {
            hba_base: AtomicU64::new(0),
            port_idx: AtomicU32::new(0),
            present: AtomicBool::new(false),
            engine_id: AtomicU32::new(0),
            // SAFETY: Mutex<AhciState> ne peut pas être const-initialisé correctement
            // sans const_default. Utilise une valeur bidon, remplacée par init().
            state: Mutex::new(unsafe { core::mem::zeroed() }),
        }
    }

    unsafe fn port_base(&self) -> *mut u32 {
        let b = self.hba_base.load(Ordering::Relaxed);
        let p = self.port_idx.load(Ordering::Relaxed) as usize;
        (b + (hba_regs::PORT_BASE + p * hba_regs::PORT_STRIDE) as u64) as *mut u32
    }

    unsafe fn port_read32(&self, offset: usize) -> u32 {
        self.port_base().add(offset / 4).read_volatile()
    }

    unsafe fn port_write32(&self, offset: usize, val: u32) {
        self.port_base().add(offset / 4).write_volatile(val);
    }

    /// Initialise le moteur AHCI sur le port `port`.
    ///
    /// `hba_base` : base MMIO BAR5 du HBA AHCI.
    ///
    /// # Safety : CPL 0, HBA MMIO mappé.
    pub unsafe fn init(&self, hba_base: u64, port: u32) -> bool {
        // Vérifie que le port est implémenté.
        let pi = (hba_base as *const u32)
            .add(hba_regs::PI / 4)
            .read_volatile();
        if pi & (1 << port) == 0 {
            return false;
        }

        self.hba_base.store(hba_base, Ordering::Release);
        self.port_idx.store(port, Ordering::Release);

        // Activer FRE + ST pour démarrer le port.
        let cmd_old = self.port_read32(port_regs::CMD);
        self.port_write32(port_regs::CMD, cmd_old | (1 << 4) | (1 << 0)); // FRE | ST

        // Activer AHCI Enable dans GHC.
        let ghc_ptr = (hba_base as *mut u32).add(hba_regs::GHC / 4);
        let ghc = ghc_ptr.read_volatile();
        ghc_ptr.write_volatile(ghc | (1 << 31)); // AHCI Enable

        let eid = DMA_STATS.register_engine();
        self.engine_id.store(eid as u32, Ordering::Relaxed);
        self.present.store(true, Ordering::Release);
        true
    }

    /// Soumet une opération Read DMA48 (lecture disque → mémoire).
    ///
    /// # Safety : `buf_phys` valide, taille alignée sur 512.
    pub unsafe fn submit_read(
        &self,
        lba: u64,
        sector_count: u16,
        buf_phys: PhysAddr,
    ) -> Option<usize> {
        self.submit_dma(lba, sector_count, buf_phys, false)
    }

    /// Soumet une opération Write DMA48 (mémoire → disque).
    ///
    /// # Safety : `buf_phys` valide, taille alignée sur 512.
    pub unsafe fn submit_write(
        &self,
        lba: u64,
        sector_count: u16,
        buf_phys: PhysAddr,
    ) -> Option<usize> {
        self.submit_dma(lba, sector_count, buf_phys, true)
    }

    unsafe fn submit_dma(
        &self,
        lba: u64,
        sector_count: u16,
        buf_phys: PhysAddr,
        write: bool,
    ) -> Option<usize> {
        if !self.present.load(Ordering::Acquire) {
            return None;
        }

        let size_bytes = sector_count as u32 * 512;
        let mut st = self.state.lock();

        // Chercher un slot libre.
        let slot = st.slots.iter().position(|s| !s.occupied)?;

        // Remplir le FIS.
        let fis = if write {
            AhciCommandFis::new_write_dma48(lba, sector_count)
        } else {
            AhciCommandFis::new_read_dma48(lba, sector_count)
        };
        st.cmd_tables[slot].cfis = fis;

        // Remplir le PRDT (1 entrée).
        st.cmd_tables[slot].prdt[0] = AhciPrdtEntry::new(buf_phys, size_bytes, false);

        // Adresse physique du command table.
        // SAFETY : les tables sont en mémoire statique kernel.
        let ctba_phys = PhysAddr::new(&st.cmd_tables[slot] as *const _ as u64);

        // Remplir le command header.
        st.cmd_headers[slot] = AhciCommandHeader::new(
            5, // FIS = 20 bytes = 5 DW
            1, // 1 PRDT entry
            ctba_phys, write,
        );

        // Marquer occupé.
        st.slots[slot] = AhciSlotStatus {
            occupied: true,
            write_op: write,
            size_bytes,
        };

        // Écrire CLB (adresse du command list).
        let cl_phys = &st.cmd_headers as *const _ as u64;
        self.port_write32(port_regs::CLB, (cl_phys & 0xFFFF_FFFF) as u32);
        self.port_write32(port_regs::CLBU, (cl_phys >> 32) as u32);

        // Lancer le slot via CI : set bit correspondant.
        self.port_write32(port_regs::CI, 1 << slot);

        drop(st);
        let eid = self.engine_id.load(Ordering::Relaxed) as usize;
        dma_stat_submit(eid);
        Some(slot)
    }

    /// Sonde les slots terminés en lisant CI.
    ///
    /// # Safety : contexte non-préemptible.
    pub unsafe fn poll(&self) -> usize {
        if !self.present.load(Ordering::Acquire) {
            return 0;
        }
        let ci = self.port_read32(port_regs::CI);
        let mut st = self.state.lock();
        let eid = self.engine_id.load(Ordering::Relaxed) as usize;
        let mut done = 0;

        for slot in 0..AHCI_SLOTS {
            if !st.slots[slot].occupied {
                continue;
            }
            // Si le bit CI est à 0 → complété.
            if ci & (1 << slot) == 0 {
                // Vérifier erreurs (TFD.ERR).
                let tfd = self.port_read32(port_regs::TFD);
                if tfd & 0x01 != 0 {
                    dma_stat_error(eid);
                } else {
                    dma_stat_complete(
                        eid,
                        st.slots[slot].size_bytes as u64,
                        st.slots[slot].write_op,
                        0,
                    );
                }
                st.slots[slot].occupied = false;
                done += 1;
            }
        }
        done
    }
}

// SAFETY: AhciDmaEngine utilise Mutex + atomics.
unsafe impl Sync for AhciDmaEngine {}

/// Instance globale du moteur AHCI DMA.
pub static AHCI_DMA: AhciDmaEngine = AhciDmaEngine::new_uninit();

/// Initialise le moteur AHCI DMA.
///
/// # Safety : HBA MMIO valide.
pub unsafe fn ahci_dma_init(hba_base: u64, port: u32) -> bool {
    AHCI_DMA.init(hba_base, port)
}

/// Soumet une lecture AHCI DMA.
///
/// # Safety : `buf_phys` valide.
pub unsafe fn ahci_dma_read(lba: u64, sectors: u16, buf: PhysAddr) -> Option<usize> {
    AHCI_DMA.submit_read(lba, sectors, buf)
}

/// Soumet une écriture AHCI DMA.
///
/// # Safety : `buf_phys` valide.
pub unsafe fn ahci_dma_write(lba: u64, sectors: u16, buf: PhysAddr) -> Option<usize> {
    AHCI_DMA.submit_write(lba, sectors, buf)
}

/// Sonde les compléments AHCI.
///
/// # Safety : contexte non-préemptible.
pub unsafe fn ahci_dma_poll() -> usize {
    AHCI_DMA.poll()
}
