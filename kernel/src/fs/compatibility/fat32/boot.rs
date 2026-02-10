//! FAT32 Boot Sector
//!
//! Boot sector (also called BPB - BIOS Parameter Block) parsing and validation.
//! The boot sector contains all the critical metadata about the FAT32 filesystem.

use alloc::sync::Arc;
use alloc::string::String;
use spin::Mutex;

use crate::fs::{FsError, FsResult};
use crate::fs::block::BlockDevice;
use crate::fs::utils::endian::*;

/// FAT32 Boot Sector (BIOS Parameter Block)
#[derive(Debug, Clone)]
pub struct Fat32BootSector {
    /// Bytes per sector (usually 512)
    pub bytes_per_sector: u16,
    /// Sectors per cluster (power of 2: 1, 2, 4, 8, 16, 32, 64, 128)
    pub sectors_per_cluster: u8,
    /// Number of reserved sectors (usually 32 for FAT32)
    pub reserved_sectors: u16,
    /// Number of FAT copies (usually 2)
    pub num_fats: u8,
    /// Root directory entries (0 for FAT32)
    pub root_entry_count: u16,
    /// Total sectors (16-bit, 0 if using 32-bit field)
    pub total_sectors_16: u16,
    /// Media type (0xF8 for fixed disk)
    pub media_type: u8,
    /// Sectors per FAT (FAT12/FAT16 only, 0 for FAT32)
    pub fat_size_16: u16,
    /// Sectors per track
    pub sectors_per_track: u16,
    /// Number of heads
    pub num_heads: u16,
    /// Hidden sectors before partition
    pub hidden_sectors: u32,
    /// Total sectors (32-bit)
    pub total_sectors_32: u32,
    /// FAT32-specific fields
    pub fat_size_32: u32,
    /// Extended flags
    pub ext_flags: u16,
    /// Filesystem version
    pub fs_version: u16,
    /// Root directory cluster
    pub root_cluster: u32,
    /// FSInfo sector
    pub fs_info: u16,
    /// Backup boot sector
    pub backup_boot_sector: u16,
    /// Drive number
    pub drive_number: u8,
    /// Extended boot signature (0x29)
    pub boot_sig: u8,
    /// Volume ID (serial number)
    pub volume_id: u32,
    /// Volume label (11 bytes)
    volume_label: [u8; 11],
    /// Filesystem type string (8 bytes, should be "FAT32   ")
    fs_type: [u8; 8],
}

impl Fat32BootSector {
    /// Read boot sector from device
    pub fn read(device: &Arc<Mutex<dyn BlockDevice>>) -> FsResult<Self> {
        let mut buf = [0u8; 512];
        {
            let dev = device.lock();
            dev.read(0, &mut buf)?;
        }

        Self::parse(&buf)
    }

    /// Parse boot sector from buffer
    pub fn parse(buf: &[u8]) -> FsResult<Self> {
        if buf.len() < 512 {
            return Err(FsError::InvalidData);
        }

        // Check boot signature
        if buf[510] != 0x55 || buf[511] != 0xAA {
            return Err(FsError::InvalidData);
        }

        // Parse BPB
        let bytes_per_sector = u16::from_le_bytes([buf[11], buf[12]]);
        let sectors_per_cluster = buf[13];
        let reserved_sectors = u16::from_le_bytes([buf[14], buf[15]]);
        let num_fats = buf[16];
        let root_entry_count = u16::from_le_bytes([buf[17], buf[18]]);
        let total_sectors_16 = u16::from_le_bytes([buf[19], buf[20]]);
        let media_type = buf[21];
        let fat_size_16 = u16::from_le_bytes([buf[22], buf[23]]);
        let sectors_per_track = u16::from_le_bytes([buf[24], buf[25]]);
        let num_heads = u16::from_le_bytes([buf[26], buf[27]]);
        let hidden_sectors = read_le_u32(&buf[28..32]);
        let total_sectors_32 = read_le_u32(&buf[32..36]);

        // FAT32-specific fields
        let fat_size_32 = read_le_u32(&buf[36..40]);
        let ext_flags = u16::from_le_bytes([buf[40], buf[41]]);
        let fs_version = u16::from_le_bytes([buf[42], buf[43]]);
        let root_cluster = read_le_u32(&buf[44..48]);
        let fs_info = u16::from_le_bytes([buf[48], buf[49]]);
        let backup_boot_sector = u16::from_le_bytes([buf[50], buf[51]]);

        let drive_number = buf[64];
        let boot_sig = buf[66];
        let volume_id = read_le_u32(&buf[67..71]);

        let mut volume_label = [0u8; 11];
        volume_label.copy_from_slice(&buf[71..82]);

        let mut fs_type = [0u8; 8];
        fs_type.copy_from_slice(&buf[82..90]);

        Ok(Self {
            bytes_per_sector,
            sectors_per_cluster,
            reserved_sectors,
            num_fats,
            root_entry_count,
            total_sectors_16,
            media_type,
            fat_size_16,
            sectors_per_track,
            num_heads,
            hidden_sectors,
            total_sectors_32,
            fat_size_32,
            ext_flags,
            fs_version,
            root_cluster,
            fs_info,
            backup_boot_sector,
            drive_number,
            boot_sig,
            volume_id,
            volume_label,
            fs_type,
        })
    }

    /// Validate boot sector
    pub fn validate(&self) -> FsResult<()> {
        // Check bytes per sector
        if self.bytes_per_sector != 512 && self.bytes_per_sector != 1024
            && self.bytes_per_sector != 2048 && self.bytes_per_sector != 4096
        {
            log::error!("FAT32: Invalid bytes per sector: {}", self.bytes_per_sector);
            return Err(FsError::InvalidData);
        }

        // Check sectors per cluster (must be power of 2)
        if self.sectors_per_cluster == 0 || (self.sectors_per_cluster & (self.sectors_per_cluster - 1)) != 0 {
            log::error!("FAT32: Invalid sectors per cluster: {}", self.sectors_per_cluster);
            return Err(FsError::InvalidData);
        }

        // Check number of FATs
        if self.num_fats == 0 || self.num_fats > 2 {
            log::error!("FAT32: Invalid number of FATs: {}", self.num_fats);
            return Err(FsError::InvalidData);
        }

        // Check FAT32 signature
        if self.boot_sig == 0x29 {
            // Extended boot signature present
            if &self.fs_type != b"FAT32   " {
                log::warn!("FAT32: Filesystem type string is not 'FAT32   ': {:?}",
                    core::str::from_utf8(&self.fs_type));
                // Don't fail - some implementations don't set this correctly
            }
        }

        // Root entry count must be 0 for FAT32
        if self.root_entry_count != 0 {
            log::error!("FAT32: Root entry count must be 0 for FAT32, got {}", self.root_entry_count);
            return Err(FsError::InvalidData);
        }

        // FAT size must be in 32-bit field
        if self.fat_size_16 != 0 {
            log::error!("FAT32: FAT size must be in 32-bit field");
            return Err(FsError::InvalidData);
        }

        if self.fat_size_32 == 0 {
            log::error!("FAT32: FAT size 32 cannot be 0");
            return Err(FsError::InvalidData);
        }

        // Check root cluster
        if self.root_cluster < 2 {
            log::error!("FAT32: Invalid root cluster: {}", self.root_cluster);
            return Err(FsError::InvalidData);
        }

        Ok(())
    }

    /// Get volume label as string
    pub fn volume_label(&self) -> String {
        let label = core::str::from_utf8(&self.volume_label)
            .unwrap_or("INVALID")
            .trim_end();
        String::from(label)
    }

    /// Get filesystem type string
    pub fn fs_type_str(&self) -> String {
        let fs_type = core::str::from_utf8(&self.fs_type)
            .unwrap_or("INVALID")
            .trim_end();
        String::from(fs_type)
    }

    /// Get total sectors
    pub fn total_sectors(&self) -> u32 {
        if self.total_sectors_16 != 0 {
            self.total_sectors_16 as u32
        } else {
            self.total_sectors_32
        }
    }

    /// Get data sectors
    pub fn data_sectors(&self) -> u32 {
        let root_dir_sectors = ((self.root_entry_count as u32 * 32) + (self.bytes_per_sector as u32 - 1))
            / self.bytes_per_sector as u32;

        let fat_size = if self.fat_size_16 != 0 {
            self.fat_size_16 as u32
        } else {
            self.fat_size_32
        };

        let total_sectors = self.total_sectors();
        let reserved = self.reserved_sectors as u32;
        let fat_sectors = self.num_fats as u32 * fat_size;

        total_sectors - (reserved + fat_sectors + root_dir_sectors)
    }

    /// Get total clusters
    pub fn total_clusters(&self) -> u32 {
        self.data_sectors() / self.sectors_per_cluster as u32
    }

    /// Determine FAT type from cluster count
    pub fn fat_type(&self) -> FatType {
        let total_clusters = self.total_clusters();

        if total_clusters < 4085 {
            FatType::Fat12
        } else if total_clusters < 65525 {
            FatType::Fat16
        } else {
            FatType::Fat32
        }
    }
}

/// FAT type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FatType {
    Fat12,
    Fat16,
    Fat32,
}

impl FatType {
    pub fn as_str(&self) -> &'static str {
        match self {
            FatType::Fat12 => "FAT12",
            FatType::Fat16 => "FAT16",
            FatType::Fat32 => "FAT32",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_boot_sector_validation() {
        let mut buf = [0u8; 512];

        // Set boot signature
        buf[510] = 0x55;
        buf[511] = 0xAA;

        // Set required fields for FAT32
        buf[11] = 0x00; // bytes_per_sector low
        buf[12] = 0x02; // bytes_per_sector high (512)
        buf[13] = 0x08; // sectors_per_cluster (8)
        buf[14] = 0x20; // reserved_sectors low (32)
        buf[15] = 0x00; // reserved_sectors high
        buf[16] = 0x02; // num_fats (2)

        // FAT size 32
        buf[36] = 0x00;
        buf[37] = 0x10;
        buf[38] = 0x00;
        buf[39] = 0x00; // 4096 sectors

        // Root cluster
        buf[44] = 0x02; // cluster 2
        buf[45] = 0x00;
        buf[46] = 0x00;
        buf[47] = 0x00;

        // Extended boot signature
        buf[66] = 0x29;

        // FS type
        buf[82..90].copy_from_slice(b"FAT32   ");

        let bs = Fat32BootSector::parse(&buf).unwrap();
        assert!(bs.validate().is_ok());
    }
}
