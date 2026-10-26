// kernel/src/fs/exofs/storage/ramdisk.rs
//
// Pilote Ramdisk BlockDevice pour Exo-OS.
//
// Permet de monter le rootfs ExoFS depuis un module mémoire Multiboot2
// (ex: Live ISO sans disque VirtIO externe attaché).

extern crate alloc;

use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::recovery::boot_recovery::BlockDevice;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;
use crate::memory::core::{phys_to_virt, PhysAddr};

pub struct RamdiskBlockDevice {
    base_virt: *mut u8,
    size_bytes: usize,
    block_size: u32,
    lock: Mutex<()>,
}

// SAFETY: L'accès aux tampons mémoire du ramdisk est protégé par `lock` pour les écritures,
// et les lectures concurrentes sont sans danger car les structures sont isolées.
unsafe impl Send for RamdiskBlockDevice {}
unsafe impl Sync for RamdiskBlockDevice {}

impl BlockDevice for RamdiskBlockDevice {
    fn read_block(&self, lba: u64, buf: &mut [u8]) -> ExofsResult<()> {
        let block_size = self.block_size as usize;
        let offset = (lba as usize)
            .checked_mul(block_size)
            .ok_or(ExofsError::IoError)?;
        if offset >= self.size_bytes {
            return Err(ExofsError::IoError);
        }
        let to_copy = buf.len().min(self.size_bytes - offset);
        unsafe {
            core::ptr::copy_nonoverlapping(self.base_virt.add(offset), buf.as_mut_ptr(), to_copy);
        }
        if to_copy < buf.len() {
            buf[to_copy..].fill(0);
        }
        Ok(())
    }

    fn write_block(&self, lba: u64, buf: &[u8]) -> ExofsResult<()> {
        let block_size = self.block_size as usize;
        let offset = (lba as usize)
            .checked_mul(block_size)
            .ok_or(ExofsError::IoError)?;
        if offset >= self.size_bytes {
            return Err(ExofsError::IoError);
        }
        let to_copy = buf.len().min(self.size_bytes - offset);
        let _g = self.lock.lock();
        unsafe {
            core::ptr::copy_nonoverlapping(buf.as_ptr(), self.base_virt.add(offset), to_copy);
        }
        Ok(())
    }

    fn block_size(&self) -> u32 {
        self.block_size
    }

    fn total_blocks(&self) -> u64 {
        (self.size_bytes / self.block_size as usize) as u64
    }

    fn flush(&self) -> ExofsResult<()> {
        Ok(())
    }
}

static RAMDISK_REGISTERED: AtomicBool = AtomicBool::new(false);

/// Initialise le ramdisk depuis le module Multiboot2 et l'enregistre comme GLOBAL_DISK.
/// Utilisé pour les Live CD/ISO lorsqu'aucun disque VirtIO ni ATA n'est présent.
pub fn init_global_disk_ramdisk() -> bool {
    if RAMDISK_REGISTERED.swap(true, Ordering::AcqRel) {
        return false;
    }
    let Some((start_phys, end_phys)) = crate::arch::x86_64::boot::multiboot2::get_multiboot2_module() else {
        return false;
    };
    if end_phys <= start_phys {
        return false;
    }
    let size_bytes = (end_phys - start_phys) as usize;
    if size_bytes < 512 {
        return false;
    }

    let virt_addr = phys_to_virt(PhysAddr::new(start_phys));
    let base_virt = virt_addr.as_u64() as *mut u8;

    let dev = Arc::new(RamdiskBlockDevice {
        base_virt,
        size_bytes,
        block_size: 4096,
        lock: Mutex::new(()),
    });

    super::virtio_adapter::register_global_disk(dev)
}
