#![no_std]
//! exo-ahci — Driver AHCI / SATA (HDD & SSD SATA) userspace pour Exo-OS.
//!
//! Driver **réel** conforme Serial ATA AHCI 1.3.1. Présente la même interface
//! bloc que `exo-virtio-blk` / `exo-nvme` (`read_block`/`write_block`/`flush`
//! sur blocs ExoFS de 4096 octets). Le kernel fournit DMA + MMIO via [`AhciHal`].
//!
//! ## Sûreté / anti-CVE
//! - I/O **synchrone** une-commande-à-la-fois → pas de course sur les slots.
//! - Une **seule** entrée PRDT par transfert, taille bornée (≤ 4 Mio, ici 4 Kio).
//! - Toutes les attentes matérielles sont **bornées** (anti-hang) : reset du
//!   moteur de commandes, BSY/DRQ, et CI.
//! - Encodage FIS/PRDT/Command-Header isolé dans [`structures`] et testé.
//! - `dma_alloc` fail-closed.

extern crate alloc;

pub mod regs;
pub mod structures;

use structures::{CmdHeader, FisRegH2D, PrdtEntry, FIS_H2D_DWORDS};

pub const EXOFS_BLOCK_SIZE: usize = 4096;
const PAGE_SIZE: usize = 4096;
const CMD_LIST_BYTES: usize = 32 * 32; // 32 headers × 32 octets
const CTBL_FIS_OFFSET: usize = 0x00;
const CTBL_PRDT_OFFSET: usize = 0x80;
const SPIN_LIMIT: u32 = 50_000_000;

#[derive(Clone, Copy, Debug)]
pub struct DmaRegion {
    pub phys: u64,
    pub virt: *mut u8,
    pub pages: usize,
}

/// Primitives matérielles injectées par le kernel (ou un mock en test).
pub trait AhciHal {
    fn dma_alloc(&self, pages: usize) -> Option<DmaRegion>;
    /// # Safety
    /// `region` doit provenir de `dma_alloc` et ne plus être référencée.
    unsafe fn dma_dealloc(&self, region: DmaRegion);
    fn mmio_read32(&self, off: usize) -> u32;
    fn mmio_write32(&self, off: usize, val: u32);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AhciError {
    DmaExhausted,
    NoSataDevice,
    PortTimeout,
    TaskFileError,
    InvalidBuffer,
    OutOfBounds,
    UnsupportedSectorSize(u32),
    PrdtBuild,
    IdentifyFailed,
}

pub struct AhciDevice<H: AhciHal> {
    hal: H,
    port: u32,
    clb: DmaRegion,   // command list (1 Kio utilisé)
    fb: DmaRegion,    // received FIS (256 octets utilisés)
    ctbl: DmaRegion,  // command table (FIS + PRDT)
    data: DmaRegion,  // buffer rebond 1 page
    ncs: u32,         // nombre de slots de commande
    sector_size: u32, // octets par secteur logique (512 ou 4096)
    capacity_sectors: u64,
}

impl<H: AhciHal> AhciDevice<H> {
    /// Initialise le HBA, détecte le premier disque SATA, le configure et fait
    /// un IDENTIFY DEVICE.
    pub fn new(hal: H) -> Result<Self, AhciError> {
        // 1. Activer le mode AHCI.
        let ghc = hal.mmio_read32(regs::HBA_GHC);
        hal.mmio_write32(regs::HBA_GHC, ghc | regs::GHC_AE);

        let cap = hal.mmio_read32(regs::HBA_CAP);
        let ncs = regs::cap_ncs(cap);
        let pi = hal.mmio_read32(regs::HBA_PI);
        let np = regs::cap_np(cap);

        // 2. Trouver le premier port avec un disque SATA.
        let mut found: Option<u32> = None;
        let mut n = 0u32;
        while n < np.min(32) {
            if pi & (1 << n) != 0 {
                let ssts = hal.mmio_read32(regs::port_reg(n, regs::PORT_SSTS));
                if regs::ssts_device_ready(ssts) {
                    let sig = hal.mmio_read32(regs::port_reg(n, regs::PORT_SIG));
                    if sig == regs::SIG_SATA {
                        found = Some(n);
                        break;
                    }
                }
            }
            n += 1;
        }
        let port = found.ok_or(AhciError::NoSataDevice)?;

        // 3. Allouer les régions DMA du port.
        let clb = alloc_zeroed(&hal, 1)?;
        let fb = alloc_zeroed(&hal, 1)?;
        let ctbl = alloc_zeroed(&hal, 1)?;
        let data = alloc_zeroed(&hal, 1)?;

        let mut dev = Self {
            hal,
            port,
            clb,
            fb,
            ctbl,
            data,
            ncs,
            sector_size: 512,
            capacity_sectors: 0,
        };

        dev.rebase()?;
        dev.identify()?;
        Ok(dev)
    }

    /// Arrête le moteur de commandes, programme CLB/FB, démarre le moteur.
    fn rebase(&mut self) -> Result<(), AhciError> {
        self.stop_cmd()?;

        // Command List Base (1 Kio aligné — page alignée le garantit).
        self.write_port(regs::PORT_CLB, self.clb.phys as u32);
        self.write_port(regs::PORT_CLBU, (self.clb.phys >> 32) as u32);
        // FIS Base (256 aligné).
        self.write_port(regs::PORT_FB, self.fb.phys as u32);
        self.write_port(regs::PORT_FBU, (self.fb.phys >> 32) as u32);

        // Effacer SERR.
        self.write_port(regs::PORT_SERR, 0xFFFF_FFFF);

        self.start_cmd()?;
        Ok(())
    }

    fn stop_cmd(&self) -> Result<(), AhciError> {
        let mut cmd = self.read_port(regs::PORT_CMD);
        cmd &= !regs::CMD_ST;
        cmd &= !regs::CMD_FRE;
        self.write_port(regs::PORT_CMD, cmd);
        // Attendre CR et FR à 0 (bornée).
        let mut spins = 0u32;
        loop {
            let c = self.read_port(regs::PORT_CMD);
            if c & (regs::CMD_CR | regs::CMD_FR) == 0 {
                return Ok(());
            }
            spins += 1;
            if spins >= SPIN_LIMIT {
                return Err(AhciError::PortTimeout);
            }
            core::hint::spin_loop();
        }
    }

    fn start_cmd(&self) -> Result<(), AhciError> {
        // Attendre CR=0 avant de (re)démarrer.
        let mut spins = 0u32;
        while self.read_port(regs::PORT_CMD) & regs::CMD_CR != 0 {
            spins += 1;
            if spins >= SPIN_LIMIT {
                return Err(AhciError::PortTimeout);
            }
            core::hint::spin_loop();
        }
        let mut cmd = self.read_port(regs::PORT_CMD);
        cmd |= regs::CMD_FRE;
        cmd |= regs::CMD_ST;
        self.write_port(regs::PORT_CMD, cmd);
        Ok(())
    }

    fn identify(&mut self) -> Result<(), AhciError> {
        let fis = FisRegH2D::identify();
        // IDENTIFY renvoie 512 octets dans le buffer data.
        self.issue_command(fis, false, 512)
            .map_err(|_| AhciError::IdentifyFailed)?;

        // SAFETY: data est une page DMA valide remplie par IDENTIFY (512 octets).
        let base = self.data.virt;
        let word = |w: usize| unsafe { read_u16(base, w * 2) };

        // Total secteurs LBA48 : words 100-103.
        let total = (word(100) as u64)
            | ((word(101) as u64) << 16)
            | ((word(102) as u64) << 32)
            | ((word(103) as u64) << 48);

        // Taille de secteur logique : word 106 bits 14 & 12.
        let w106 = word(106);
        let sector_size = if (w106 & (1 << 14)) != 0 && (w106 & (1 << 12)) != 0 {
            let words_per_sector = (word(117) as u32) | ((word(118) as u32) << 16);
            words_per_sector.saturating_mul(2)
        } else {
            512
        };
        let sector_size = if sector_size == 0 { 512 } else { sector_size };
        if EXOFS_BLOCK_SIZE % (sector_size as usize) != 0 {
            return Err(AhciError::UnsupportedSectorSize(sector_size));
        }
        self.sector_size = sector_size;
        self.capacity_sectors = total;
        Ok(())
    }

    /// Trouve un slot de commande libre (CI et SACT à 0).
    fn find_free_slot(&self) -> Option<u32> {
        let used = self.read_port(regs::PORT_CI) | self.read_port(regs::PORT_SACT);
        let mut slot = 0u32;
        while slot < self.ncs {
            if used & (1 << slot) == 0 {
                return Some(slot);
            }
            slot += 1;
        }
        None
    }

    /// Construit la commande dans un slot, l'émet, attend sa complétion.
    fn issue_command(
        &mut self,
        fis: FisRegH2D,
        write: bool,
        byte_count: usize,
    ) -> Result<(), AhciError> {
        let slot = self.find_free_slot().ok_or(AhciError::PortTimeout)?;

        // 1. PRDT : 0 entrée pour une commande sans données (ex. FLUSH), sinon 1
        //    entrée vers le buffer data.
        let prdtl: u16 = if byte_count == 0 { 0 } else { 1 };
        // SAFETY: ctbl est une page DMA ; offsets dans la page.
        unsafe {
            // FIS dans la command table.
            core::ptr::write_volatile(self.ctbl.virt.add(CTBL_FIS_OFFSET) as *mut FisRegH2D, fis);
            if prdtl == 1 {
                let prdt =
                    PrdtEntry::new(self.data.phys, byte_count, true).ok_or(AhciError::PrdtBuild)?;
                core::ptr::write_volatile(
                    self.ctbl.virt.add(CTBL_PRDT_OFFSET) as *mut PrdtEntry,
                    prdt,
                );
            }
        }

        // 2. Command header[slot] : CFL=5, W, PRDTL, CTBA=ctbl.
        let header = CmdHeader::new(FIS_H2D_DWORDS, write, prdtl, self.ctbl.phys);
        // SAFETY: clb est une page DMA ; slot < 32.
        unsafe {
            core::ptr::write_volatile(
                (self.clb.virt as *mut CmdHeader).add(slot as usize),
                header,
            );
        }

        // 3. Effacer les interruptions du port et attendre que le device soit prêt.
        self.write_port(regs::PORT_IS, 0xFFFF_FFFF);
        self.wait_not_busy()?;

        // 4. Émettre la commande.
        self.write_port(regs::PORT_CI, 1 << slot);

        // 5. Attendre la complétion (CI bit cleared) + vérifier l'erreur.
        let mut spins = 0u32;
        loop {
            if self.read_port(regs::PORT_CI) & (1 << slot) == 0 {
                break;
            }
            if self.read_port(regs::PORT_IS) & regs::IS_TFES != 0 {
                return Err(AhciError::TaskFileError);
            }
            spins += 1;
            if spins >= SPIN_LIMIT {
                return Err(AhciError::PortTimeout);
            }
            core::hint::spin_loop();
        }
        if self.read_port(regs::PORT_IS) & regs::IS_TFES != 0 {
            return Err(AhciError::TaskFileError);
        }
        Ok(())
    }

    fn wait_not_busy(&self) -> Result<(), AhciError> {
        let mut spins = 0u32;
        while regs::tfd_busy(self.read_port(regs::PORT_TFD)) {
            spins += 1;
            if spins >= SPIN_LIMIT {
                return Err(AhciError::PortTimeout);
            }
            core::hint::spin_loop();
        }
        Ok(())
    }

    // ── Surface bloc ─────────────────────────────────────────────────────────

    pub fn block_size(&self) -> u32 {
        EXOFS_BLOCK_SIZE as u32
    }

    fn sectors_per_block(&self) -> u64 {
        (EXOFS_BLOCK_SIZE as u64) / (self.sector_size as u64)
    }

    pub fn total_blocks(&self) -> u64 {
        let spb = self.sectors_per_block();
        if spb == 0 {
            0
        } else {
            self.capacity_sectors / spb
        }
    }

    fn block_to_lba(&self, block_id: u64) -> Result<(u64, u16), AhciError> {
        if block_id >= self.total_blocks() {
            return Err(AhciError::OutOfBounds);
        }
        let spb = self.sectors_per_block();
        let lba = block_id.checked_mul(spb).ok_or(AhciError::OutOfBounds)?;
        Ok((lba, spb as u16))
    }

    pub fn read_block(&mut self, block_id: u64, buf: &mut [u8]) -> Result<(), AhciError> {
        if buf.len() != EXOFS_BLOCK_SIZE {
            return Err(AhciError::InvalidBuffer);
        }
        let (lba, sectors) = self.block_to_lba(block_id)?;
        let fis = FisRegH2D::read_write(false, lba, sectors);
        self.issue_command(fis, false, EXOFS_BLOCK_SIZE)?;
        // SAFETY: data est une page DMA de 4096 octets remplie par le read.
        unsafe {
            core::ptr::copy_nonoverlapping(self.data.virt, buf.as_mut_ptr(), EXOFS_BLOCK_SIZE);
        }
        Ok(())
    }

    pub fn write_block(&mut self, block_id: u64, buf: &[u8]) -> Result<(), AhciError> {
        if buf.len() != EXOFS_BLOCK_SIZE {
            return Err(AhciError::InvalidBuffer);
        }
        let (lba, sectors) = self.block_to_lba(block_id)?;
        // SAFETY: data est une page DMA de 4096 octets.
        unsafe {
            core::ptr::copy_nonoverlapping(buf.as_ptr(), self.data.virt, EXOFS_BLOCK_SIZE);
        }
        let fis = FisRegH2D::read_write(true, lba, sectors);
        self.issue_command(fis, true, EXOFS_BLOCK_SIZE)
    }

    pub fn flush(&mut self) -> Result<(), AhciError> {
        let fis = FisRegH2D::flush();
        // FLUSH ne transfère pas de données → PRDTL=0 (byte_count=0).
        self.issue_command(fis, false, 0)
    }

    // ── Accès registres port ─────────────────────────────────────────────────

    #[inline]
    fn read_port(&self, reg: usize) -> u32 {
        self.hal.mmio_read32(regs::port_reg(self.port, reg))
    }
    #[inline]
    fn write_port(&self, reg: usize, val: u32) {
        self.hal.mmio_write32(regs::port_reg(self.port, reg), val);
    }
}

impl<H: AhciHal> Drop for AhciDevice<H> {
    fn drop(&mut self) {
        let _ = self.stop_cmd();
        // SAFETY: les régions proviennent de dma_alloc et ne sont plus utilisées.
        unsafe {
            self.hal.dma_dealloc(self.clb);
            self.hal.dma_dealloc(self.fb);
            self.hal.dma_dealloc(self.ctbl);
            self.hal.dma_dealloc(self.data);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn alloc_zeroed<H: AhciHal>(hal: &H, pages: usize) -> Result<DmaRegion, AhciError> {
    let region = hal.dma_alloc(pages).ok_or(AhciError::DmaExhausted)?;
    let _ = CMD_LIST_BYTES; // documente la taille de la liste de commandes
    // SAFETY: dma_alloc garantit pages*PAGE_SIZE octets valides à virt.
    unsafe {
        core::ptr::write_bytes(region.virt, 0, pages * PAGE_SIZE);
    }
    Ok(region)
}

#[inline]
unsafe fn read_u16(base: *const u8, off: usize) -> u16 {
    let lo = core::ptr::read_volatile(base.add(off));
    let hi = core::ptr::read_volatile(base.add(off + 1));
    (lo as u16) | ((hi as u16) << 8)
}

#[cfg(test)]
mod tests;
