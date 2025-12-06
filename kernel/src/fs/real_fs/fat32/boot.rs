//! FAT32 Boot Sector
//!
//! Parse et valide le boot sector FAT32

use crate::drivers::block::BlockDevice;
use crate::fs::{FsError, FsResult};
use alloc::sync::Arc;
use spin::Mutex;

/// FAT32 Boot Sector (512 bytes)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Fat32BootSector {
    pub jmp: [u8; 3],
    pub oem_name: [u8; 8],
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub reserved_sectors: u16,
    pub fat_count: u8,
    pub root_entries: u16,          // 0 pour FAT32
    pub total_sectors_16: u16,      // 0 pour FAT32
    pub media_type: u8,
    pub sectors_per_fat_16: u16,    // 0 pour FAT32
    pub sectors_per_track: u16,
    pub head_count: u16,
    pub hidden_sectors: u32,
    pub total_sectors_32: u32,
    // FAT32 Extended BPB
    pub sectors_per_fat: u32,
    pub flags: u16,
    pub version: u16,
    pub root_cluster: u32,
    pub fsinfo_sector: u16,
    pub backup_boot_sector: u16,
    pub reserved: [u8; 12],
    pub drive_number: u8,
    pub reserved1: u8,
    pub boot_signature: u8,
    pub volume_id: u32,
    pub volume_label: [u8; 11],
    pub fs_type: [u8; 8],           // "FAT32   "
}

impl Fat32BootSector {
    /// Lit et parse le boot sector
    pub fn read(device: &Arc<Mutex<dyn BlockDevice>>) -> FsResult<Self> {
        let mut buffer = [0u8; 512];
        
        device.lock().read(0, &mut buffer)
            .map_err(|_| FsError::IoError)?;
        
        // Valide signature
        if buffer[510] != 0x55 || buffer[511] != 0xAA {
            return Err(FsError::InvalidData);
        }
        
        let boot = unsafe {
            core::ptr::read_unaligned(buffer.as_ptr() as *const Fat32BootSector)
        };
        
        // Valide FAT32
        boot.validate()?;
        
        Ok(boot)
    }
    
    /// Valide que c'est bien FAT32
    fn validate(&self) -> FsResult<()> {
        // Check bytes per sector (512, 1024, 2048, 4096)
        if !matches!(self.bytes_per_sector, 512 | 1024 | 2048 | 4096) {
            return Err(FsError::InvalidData);
        }
        
        // Check sectors per cluster (power of 2, 1-128)
        if self.sectors_per_cluster == 0 || self.sectors_per_cluster > 128 {
            return Err(FsError::InvalidData);
        }
        
        if !self.sectors_per_cluster.is_power_of_two() {
            return Err(FsError::InvalidData);
        }
        
        // Check FAT count (1 ou 2)
        if self.fat_count == 0 || self.fat_count > 2 {
            return Err(FsError::InvalidData);
        }
        
        // FAT32 specific
        if self.root_entries != 0 {
            return Err(FsError::InvalidData);
        }
        
        if self.total_sectors_16 != 0 {
            return Err(FsError::InvalidData);
        }
        
        if self.sectors_per_fat_16 != 0 {
            return Err(FsError::InvalidData);
        }
        
        // Check root cluster (>= 2)
        if self.root_cluster < 2 {
            return Err(FsError::InvalidData);
        }
        
        Ok(())
    }
}

/// FS Info structure (sector 1)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct FsInfoSector {
    pub lead_signature: u32,        // 0x41615252
    pub reserved1: [u8; 480],
    pub struct_signature: u32,      // 0x61417272
    pub free_clusters: u32,
    pub next_free: u32,
    pub reserved2: [u8; 12],
    pub trail_signature: u32,       // 0xAA550000
}

impl super::FsInfo {
    /// Lit FS Info depuis le disque
    pub fn read(device: &Arc<Mutex<dyn BlockDevice>>, sector: u16, bytes_per_sector: u16) -> FsResult<Self> {
        let mut buffer = alloc::vec![0u8; bytes_per_sector as usize];
        
        device.lock().read(sector as u64, &mut buffer)
            .map_err(|_| FsError::IoError)?;
        
        let fsinfo = unsafe {
            core::ptr::read_unaligned(buffer.as_ptr() as *const FsInfoSector)
        };
        
        // Valide signatures
        if fsinfo.lead_signature != 0x41615252 {
            return Err(FsError::InvalidData);
        }
        
        if fsinfo.struct_signature != 0x61417272 {
            return Err(FsError::InvalidData);
        }
        
        if fsinfo.trail_signature != 0xAA550000 {
            return Err(FsError::InvalidData);
        }
        
        Ok(Self {
            free_clusters: fsinfo.free_clusters,
            next_free: fsinfo.next_free,
        })
    }
    
    /// Écrit FS Info vers le disque
    pub fn write(&self, device: &Arc<Mutex<dyn BlockDevice>>, sector: u16, bytes_per_sector: u16) -> FsResult<()> {
        let mut buffer = alloc::vec![0u8; bytes_per_sector as usize];
        
        let fsinfo = FsInfoSector {
            lead_signature: 0x41615252,
            reserved1: [0; 480],
            struct_signature: 0x61417272,
            free_clusters: self.free_clusters,
            next_free: self.next_free,
            reserved2: [0; 12],
            trail_signature: 0xAA550000,
        };
        
        unsafe {
            core::ptr::write_unaligned(buffer.as_mut_ptr() as *mut FsInfoSector, fsinfo);
        }
        
        device.lock().write(sector as u64, &buffer)
            .map_err(|_| FsError::IoError)?;
        
        Ok(())
    }
}
