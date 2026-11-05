// kernel/src/memory/dma/engines/ahci_dma.rs
//
// SATA AHCI DMA engine (Advanced Host Controller Interface).
// Ref: AHCI 1.3.1 Specification.
// Layer 0 — no_std, MMIO access exclusively via raw pointers.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

use crate::memory::core::PhysAddr;
use crate::memory::dma::stats::counters::{
    dma_stat_complete, dma_stat_error, dma_stat_submit, DMA_STATS,
};

// ─────────────────────────────────────────────────────────────────────────────
// AHCI HBA REGISTERS
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

    /// Base of each port (0x100 + port * 0x80).
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
// AHCI STRUCTURES
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
    /// ATA command.
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
        // SAFETY: repr(C), all fields are integers/arrays, 0 is valid.
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
    /// PRD Byte Count (cleared by HW).
    pub prdbc: u32,
    /// Command Table Base Address (low, 128-byte aligned).
    pub ctba_lo: u32,
    /// Command Table Base Address (high).
    pub ctba_hi: u32,
    pub _rsvd: [u32; 4],
}

impl AhciCommandHeader {
    /// `write` = true for Write DMA, false for Read DMA.
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

/// Max size of the internal command table (1 PRDT entry).
const MAX_PRDT: usize = 8;

/// Command Table (FIS + PRDT).
#[repr(C, align(128))]
#[derive(Clone, Copy)]
struct AhciCmdTable {
    cfis: AhciCommandFis,
    acmd: [u8; 16], // ATAPI command (unused for DMA)
    _rsvd: [u8; 48],
    prdt: [AhciPrdtEntry; MAX_PRDT],
}

// ─────────────────────────────────────────────────────────────────────────────
// INTERNAL SLOT
// ─────────────────────────────────────────────────────────────────────────────

const AHCI_SLOTS: usize = 32;

#[derive(Clone, Copy, Default)]
struct AhciSlotStatus {
    occupied: bool,
    write_op: bool,
    size_bytes: u32,
}

// ─────────────────────────────────────────────────────────────────────────────
// AHCI DMA ENGINE
// ─────────────────────────────────────────────────────────────────────────────

struct AhciState {
    cmd_headers: [AhciCommandHeader; AHCI_SLOTS],
    cmd_tables: [AhciCmdTable; AHCI_SLOTS],
    slots: [AhciSlotStatus; AHCI_SLOTS],
}

impl AhciState {
    #[allow(dead_code)]
    fn new() -> Self {
        // SAFETY: all fields represented with 0.
        unsafe {
            AhciState {
                cmd_headers: core::mem::zeroed(),
                cmd_tables: core::mem::zeroed(),
                slots: [AhciSlotStatus::default(); AHCI_SLOTS],
            }
        }
    }
}

/// AHCI DMA engine.
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
            // SAFETY: Mutex<AhciState> cannot be const-initialized properly
            // without const_default. Uses a dummy value, replaced by init().
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

    /// Initializes the AHCI engine on port `port`.
    ///
    /// `hba_base`: MMIO BAR5 base of the AHCI HBA.
    ///
    /// # Safety
    /// CPL 0, HBA MMIO mapped.
    pub unsafe fn init(&self, hba_base: u64, port: u32) -> bool {
        // Verify that the port is implemented.
        let pi = (hba_base as *const u32)
            .add(hba_regs::PI / 4)
            .read_volatile();
        if pi & (1 << port) == 0 {
            return false;
        }

        self.hba_base.store(hba_base, Ordering::Release);
        self.port_idx.store(port, Ordering::Release);

        // Enable FRE + ST to start the port.
        let cmd_old = self.port_read32(port_regs::CMD);
        self.port_write32(port_regs::CMD, cmd_old | (1 << 4) | (1 << 0)); // FRE | ST

        // Enable AHCI Enable in GHC.
        let ghc_ptr = (hba_base as *mut u32).add(hba_regs::GHC / 4);
        let ghc = ghc_ptr.read_volatile();
        ghc_ptr.write_volatile(ghc | (1 << 31)); // AHCI Enable

        let eid = DMA_STATS.register_engine();
        self.engine_id.store(eid as u32, Ordering::Relaxed);
        self.present.store(true, Ordering::Release);
        true
    }

    /// Submits a Read DMA48 operation (disk read -> memory).
    ///
    /// # Safety
    /// `buf_phys` valid, size aligned to 512.
    pub unsafe fn submit_read(
        &self,
        lba: u64,
        sector_count: u16,
        buf_phys: PhysAddr,
    ) -> Option<usize> {
        self.submit_dma(lba, sector_count, buf_phys, false)
    }

    /// Submits a Write DMA48 operation (memory -> disk).
    ///
    /// # Safety
    /// `buf_phys` valid, size aligned to 512.
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

        // Find a free slot.
        let slot = st.slots.iter().position(|s| !s.occupied)?;

        // Fill the FIS.
        let fis = if write {
            AhciCommandFis::new_write_dma48(lba, sector_count)
        } else {
            AhciCommandFis::new_read_dma48(lba, sector_count)
        };
        st.cmd_tables[slot].cfis = fis;

        // Fill the PRDT (1 entry).
        st.cmd_tables[slot].prdt[0] = AhciPrdtEntry::new(buf_phys, size_bytes, false);

        // Physical address of the command table.
        // SAFETY: tables are in kernel static memory.
        let ctba_phys = PhysAddr::new(&st.cmd_tables[slot] as *const _ as u64);

        // Fill the command header.
        st.cmd_headers[slot] = AhciCommandHeader::new(
            5, // FIS = 20 bytes = 5 DW
            1, // 1 PRDT entry
            ctba_phys, write,
        );

        // Mark occupied.
        st.slots[slot] = AhciSlotStatus {
            occupied: true,
            write_op: write,
            size_bytes,
        };

        // Write CLB (command list address).
        let cl_phys = &st.cmd_headers as *const _ as u64;
        self.port_write32(port_regs::CLB, (cl_phys & 0xFFFF_FFFF) as u32);
        self.port_write32(port_regs::CLBU, (cl_phys >> 32) as u32);

        // Launch the slot via CI: set corresponding bit.
        self.port_write32(port_regs::CI, 1 << slot);

        drop(st);
        let eid = self.engine_id.load(Ordering::Relaxed) as usize;
        dma_stat_submit(eid);
        Some(slot)
    }

    /// Polls completed slots by reading CI.
    ///
    /// # Safety
    /// Non-preemptible context.
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
            // If CI bit is 0 -> completed.
            if ci & (1 << slot) == 0 {
                // Check errors (TFD.ERR).
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

// SAFETY: AhciDmaEngine uses Mutex + atomics.
unsafe impl Sync for AhciDmaEngine {}

/// Global instance of the AHCI DMA engine.
pub static AHCI_DMA: AhciDmaEngine = AhciDmaEngine::new_uninit();

/// Initializes the AHCI DMA engine.
///
/// # Safety
/// Valid HBA MMIO.
pub unsafe fn ahci_dma_init(hba_base: u64, port: u32) -> bool {
    AHCI_DMA.init(hba_base, port)
}

/// Submits an AHCI DMA read.
///
/// # Safety
/// `buf_phys` valid.
pub unsafe fn ahci_dma_read(lba: u64, sectors: u16, buf: PhysAddr) -> Option<usize> {
    AHCI_DMA.submit_read(lba, sectors, buf)
}

/// Submits an AHCI DMA write.
///
/// # Safety
/// `buf_phys` valid.
pub unsafe fn ahci_dma_write(lba: u64, sectors: u16, buf: PhysAddr) -> Option<usize> {
    AHCI_DMA.submit_write(lba, sectors, buf)
}

/// Polls AHCI completions.
///
/// # Safety
/// Non-preemptible context.
pub unsafe fn ahci_dma_poll() -> usize {
    AHCI_DMA.poll()
}
