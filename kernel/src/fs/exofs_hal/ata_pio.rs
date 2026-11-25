//! ATA PIO legacy IDE block adapter (primary channel master, port 0x1F0).
//!
//! Lives in `fs/exofs_hal/` so `fs/exofs/` remains free of direct x86_64 I/O
//! port code. All port operations go through `crate::arch::x86_64::{inb, inw, outb, outw}`.

extern crate alloc;

use crate::arch::x86_64::{inb, inw, outb, outw};
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::recovery::boot_recovery::BlockDevice;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

const ATA_IO: u16 = 0x1F0;
const ATA_CTRL: u16 = 0x3F6;

const REG_DATA: u16 = 0;
const REG_SECCOUNT: u16 = 2;
const REG_LBA0: u16 = 3;
const REG_LBA1: u16 = 4;
const REG_LBA2: u16 = 5;
const REG_DRIVE: u16 = 6;
const REG_STATUS: u16 = 7;
const REG_CMD: u16 = 7;

const ST_BSY: u8 = 0x80;
const ST_DRQ: u8 = 0x08;
const ST_ERR: u8 = 0x01;
const ST_DF: u8 = 0x20;

const CMD_READ_PIO: u8 = 0x20;
const CMD_WRITE_PIO: u8 = 0x30;
const CMD_IDENTIFY: u8 = 0xEC;
const CMD_CACHE_FLUSH: u8 = 0xE7;

const EXOFS_BLOCK_SIZE: usize = 4096;
const ATA_SECTOR: usize = 512;
const SECTORS_PER_BLOCK: u64 = (EXOFS_BLOCK_SIZE / ATA_SECTOR) as u64;

const SPIN_TIMEOUT: u32 = 50_000_000;

#[inline(always)]
unsafe fn io_wait() {
    let mut i = 0;
    while i < 4 {
        let _ = inb(ATA_CTRL);
        i += 1;
    }
}

unsafe fn wait_bsy_clear() {
    let mut spins = 0u32;
    while inb(ATA_IO + REG_STATUS) & ST_BSY != 0 {
        spins += 1;
        if spins > SPIN_TIMEOUT {
            break;
        }
    }
}

unsafe fn wait_drq() -> Result<(), ()> {
    let mut spins = 0u32;
    loop {
        let st = inb(ATA_IO + REG_STATUS);
        if st & ST_BSY == 0 {
            if st & (ST_ERR | ST_DF) != 0 {
                return Err(());
            }
            if st & ST_DRQ != 0 {
                return Ok(());
            }
        }
        spins += 1;
        if spins > SPIN_TIMEOUT {
            return Err(());
        }
    }
}

unsafe fn ata_setup(lba: u32, count: u8, cmd: u8) {
    wait_bsy_clear();
    outb(ATA_IO + REG_DRIVE, 0xE0 | (((lba >> 24) & 0x0F) as u8));
    io_wait();
    outb(ATA_IO + REG_SECCOUNT, count);
    outb(ATA_IO + REG_LBA0, (lba & 0xFF) as u8);
    outb(ATA_IO + REG_LBA1, ((lba >> 8) & 0xFF) as u8);
    outb(ATA_IO + REG_LBA2, ((lba >> 16) & 0xFF) as u8);
    outb(ATA_IO + REG_CMD, cmd);
}

unsafe fn ata_read_sectors(lba: u32, count: u8, buf: &mut [u8]) -> Result<(), ()> {
    if buf.len() < (count as usize) * ATA_SECTOR {
        return Err(());
    }
    ata_setup(lba, count, CMD_READ_PIO);
    let mut off = 0usize;
    let mut s = 0u8;
    while s < count {
        wait_drq()?;
        let mut w = 0;
        while w < ATA_SECTOR / 2 {
            let word = inw(ATA_IO + REG_DATA);
            buf[off] = (word & 0xFF) as u8;
            buf[off + 1] = (word >> 8) as u8;
            off += 2;
            w += 1;
        }
        s += 1;
    }
    Ok(())
}

unsafe fn ata_write_sectors(lba: u32, count: u8, buf: &[u8]) -> Result<(), ()> {
    if buf.len() < (count as usize) * ATA_SECTOR {
        return Err(());
    }
    ata_setup(lba, count, CMD_WRITE_PIO);
    let mut off = 0usize;
    let mut s = 0u8;
    while s < count {
        wait_drq()?;
        let mut w = 0;
        while w < ATA_SECTOR / 2 {
            let word = (buf[off] as u16) | ((buf[off + 1] as u16) << 8);
            outw(ATA_IO + REG_DATA, word);
            off += 2;
            w += 1;
        }
        s += 1;
    }
    outb(ATA_IO + REG_CMD, CMD_CACHE_FLUSH);
    wait_bsy_clear();
    Ok(())
}

unsafe fn ata_identify() -> u64 {
    wait_bsy_clear();
    outb(ATA_IO + REG_DRIVE, 0xE0);
    io_wait();
    outb(ATA_IO + REG_SECCOUNT, 0);
    outb(ATA_IO + REG_LBA0, 0);
    outb(ATA_IO + REG_LBA1, 0);
    outb(ATA_IO + REG_LBA2, 0);
    outb(ATA_IO + REG_CMD, CMD_IDENTIFY);
    if inb(ATA_IO + REG_STATUS) == 0 {
        return 0;
    }
    if wait_drq().is_err() {
        return 0;
    }
    let mut id = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        id[i] = inw(ATA_IO + REG_DATA);
        i += 1;
    }
    (id[60] as u64) | ((id[61] as u64) << 16)
}

pub struct AtaPioDisk {
    total_sectors: u64,
    lock: Mutex<()>,
}

// SAFETY: all IDE port accesses are serialized by `lock`.
unsafe impl Send for AtaPioDisk {}
unsafe impl Sync for AtaPioDisk {}

impl BlockDevice for AtaPioDisk {
    fn read_block(&self, lba: u64, buf: &mut [u8]) -> ExofsResult<()> {
        if buf.len() != EXOFS_BLOCK_SIZE {
            return Err(ExofsError::InvalidSize);
        }
        let _g = self.lock.lock();
        let ata_lba = (lba * SECTORS_PER_BLOCK) as u32;
        unsafe { ata_read_sectors(ata_lba, SECTORS_PER_BLOCK as u8, buf) }
            .map_err(|_| ExofsError::IoError)
    }

    fn write_block(&self, lba: u64, buf: &[u8]) -> ExofsResult<()> {
        if buf.len() != EXOFS_BLOCK_SIZE {
            return Err(ExofsError::InvalidSize);
        }
        let _g = self.lock.lock();
        let ata_lba = (lba * SECTORS_PER_BLOCK) as u32;
        unsafe { ata_write_sectors(ata_lba, SECTORS_PER_BLOCK as u8, buf) }
            .map_err(|_| ExofsError::IoError)
    }

    fn block_size(&self) -> u32 {
        EXOFS_BLOCK_SIZE as u32
    }

    fn total_blocks(&self) -> u64 {
        self.total_sectors / SECTORS_PER_BLOCK
    }

    fn flush(&self) -> ExofsResult<()> {
        Ok(())
    }
}

static ATA_REGISTERED: AtomicBool = AtomicBool::new(false);

/// Probes the legacy primary-master IDE disk (PIO) and registers it as `GLOBAL_DISK`.
pub fn init_global_disk_ata() -> bool {
    if ATA_REGISTERED.swap(true, Ordering::AcqRel) {
        return false;
    }
    let sectors = unsafe { ata_identify() };
    if sectors == 0 {
        return false;
    }
    let disk: Arc<dyn BlockDevice> = Arc::new(AtaPioDisk {
        total_sectors: sectors,
        lock: Mutex::new(()),
    });
    super::virtio_adapter::register_global_disk(disk)
}
