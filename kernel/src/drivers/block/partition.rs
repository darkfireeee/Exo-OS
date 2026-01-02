//! Partition Table Support
//!
//! Supports:
//! - MBR (Master Boot Record)
//! - GPT (GUID Partition Table)
//! - Auto-detection
//! - Partition enumeration

use alloc::vec::Vec;
use alloc::string::String;
use super::{BlockDevice, DriverResult, DriverError};

/// Partition type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionType {
    /// Empty/unused
    Empty,
    
    /// FAT12
    Fat12,
    
    /// FAT16
    Fat16,
    
    /// FAT32
    Fat32,
    
    /// NTFS
    Ntfs,
    
    /// ext2/ext3/ext4
    Ext,
    
    /// Linux swap
    LinuxSwap,
    
    /// EFI System Partition
    Efi,
    
    /// Unknown
    Unknown(u8),
}

impl From<u8> for PartitionType {
    fn from(type_id: u8) -> Self {
        match type_id {
            0x00 => PartitionType::Empty,
            0x01 => PartitionType::Fat12,
            0x04 | 0x06 | 0x0E => PartitionType::Fat16,
            0x0B | 0x0C => PartitionType::Fat32,
            0x07 => PartitionType::Ntfs,
            0x83 => PartitionType::Ext,
            0x82 => PartitionType::LinuxSwap,
            0xEF => PartitionType::Efi,
            _ => PartitionType::Unknown(type_id),
        }
    }
}

/// Partition information
#[derive(Debug, Clone)]
pub struct Partition {
    /// Partition number (0-based)
    pub number: usize,
    
    /// Partition type
    pub partition_type: PartitionType,
    
    /// Start sector (LBA)
    pub start_lba: u64,
    
    /// Size in sectors
    pub size_sectors: u64,
    
    /// Partition name/label (GPT only)
    pub name: Option<String>,
    
    /// Bootable flag (MBR only)
    pub bootable: bool,
}

impl Partition {
    /// Get partition size in bytes
    pub fn size_bytes(&self) -> u64 {
        self.size_sectors * 512
    }
    
    /// Check if partition is empty
    pub fn is_empty(&self) -> bool {
        self.partition_type == PartitionType::Empty || self.size_sectors == 0
    }
}

/// MBR Partition Entry (16 bytes)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct MbrPartitionEntry {
    bootable: u8,
    start_chs: [u8; 3],
    partition_type: u8,
    end_chs: [u8; 3],
    start_lba: u32,
    size_sectors: u32,
}

/// MBR Structure (512 bytes)
#[repr(C, packed)]
struct MbrHeader {
    bootloader: [u8; 446],
    partitions: [MbrPartitionEntry; 4],
    signature: u16,
}

/// GPT Header (92 bytes minimum)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct GptHeader {
    signature: [u8; 8],
    revision: u32,
    header_size: u32,
    crc32: u32,
    reserved: u32,
    current_lba: u64,
    backup_lba: u64,
    first_usable_lba: u64,
    last_usable_lba: u64,
    disk_guid: [u8; 16],
    partition_entries_lba: u64,
    num_partition_entries: u32,
    partition_entry_size: u32,
    partition_array_crc32: u32,
}

/// GPT Partition Entry (128 bytes)
#[repr(C, packed)]
struct GptPartitionEntry {
    partition_type_guid: [u8; 16],
    unique_partition_guid: [u8; 16],
    starting_lba: u64,
    ending_lba: u64,
    attributes: u64,
    partition_name: [u16; 36], // UTF-16LE
}

/// Partition table type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionTableType {
    Mbr,
    Gpt,
    Unknown,
}

/// Parse partition table
pub fn parse_partitions<D: BlockDevice>(device: &mut D) -> DriverResult<Vec<Partition>> {
    // Read first sector (MBR/GPT protective MBR)
    let mut buffer = vec![0u8; 512];
    device.read(0, &mut buffer)?;
    
    // Check MBR signature
    if buffer[510] != 0x55 || buffer[511] != 0xAA {
        return Err(DriverError::NotSupported);
    }
    
    // Check for GPT
    if is_gpt_protective_mbr(&buffer) {
        parse_gpt(device)
    } else {
        parse_mbr(&buffer)
    }
}

/// Check if MBR is GPT protective MBR
fn is_gpt_protective_mbr(buffer: &[u8]) -> bool {
    // First partition type should be 0xEE for GPT
    buffer[450] == 0xEE
}

/// Parse MBR partition table
fn parse_mbr(buffer: &[u8]) -> DriverResult<Vec<Partition>> {
    let mut partitions = Vec::new();
    
    // Parse 4 primary partitions
    for i in 0..4 {
        let offset = 446 + (i * 16);
        
        let bootable = buffer[offset];
        let partition_type = buffer[offset + 4];
        let start_lba = u32::from_le_bytes([
            buffer[offset + 8],
            buffer[offset + 9],
            buffer[offset + 10],
            buffer[offset + 11],
        ]) as u64;
        let size_sectors = u32::from_le_bytes([
            buffer[offset + 12],
            buffer[offset + 13],
            buffer[offset + 14],
            buffer[offset + 15],
        ]) as u64;
        
        if partition_type != 0x00 && size_sectors > 0 {
            partitions.push(Partition {
                number: i,
                partition_type: PartitionType::from(partition_type),
                start_lba,
                size_sectors,
                name: None,
                bootable: bootable == 0x80,
            });
        }
    }
    
    Ok(partitions)
}

/// Parse GPT partition table
fn parse_gpt<D: BlockDevice>(device: &mut D) -> DriverResult<Vec<Partition>> {
    // Read GPT header (LBA 1)
    let mut header_buf = vec![0u8; 512];
    device.read(1, &mut header_buf)?;
    
    // Verify GPT signature "EFI PART"
    if &header_buf[0..8] != b"EFI PART" {
        return Err(DriverError::NotSupported);
    }
    
    let num_entries = u32::from_le_bytes([
        header_buf[80],
        header_buf[81],
        header_buf[82],
        header_buf[83],
    ]) as usize;
    
    let entry_size = u32::from_le_bytes([
        header_buf[84],
        header_buf[85],
        header_buf[86],
        header_buf[87],
    ]) as usize;
    
    let entries_lba = u64::from_le_bytes([
        header_buf[72],
        header_buf[73],
        header_buf[74],
        header_buf[75],
        header_buf[76],
        header_buf[77],
        header_buf[78],
        header_buf[79],
    ]);
    
    // Read partition entries (usually starts at LBA 2)
    let entries_sectors = ((num_entries * entry_size) + 511) / 512;
    let mut entries_buf = vec![0u8; entries_sectors * 512];
    
    for i in 0..entries_sectors {
        let sector = entries_lba + i as u64;
        device.read(sector, &mut entries_buf[i * 512..(i + 1) * 512])?;
    }
    
    let mut partitions = Vec::new();
    
    // Parse each partition entry
    for i in 0..num_entries {
        let offset = i * entry_size;
        
        // Check if partition type GUID is all zeros (unused entry)
        let mut is_empty = true;
        for j in 0..16 {
            if entries_buf[offset + j] != 0 {
                is_empty = false;
                break;
            }
        }
        
        if is_empty {
            continue;
        }
        
        let starting_lba = u64::from_le_bytes([
            entries_buf[offset + 32],
            entries_buf[offset + 33],
            entries_buf[offset + 34],
            entries_buf[offset + 35],
            entries_buf[offset + 36],
            entries_buf[offset + 37],
            entries_buf[offset + 38],
            entries_buf[offset + 39],
        ]);
        
        let ending_lba = u64::from_le_bytes([
            entries_buf[offset + 40],
            entries_buf[offset + 41],
            entries_buf[offset + 42],
            entries_buf[offset + 43],
            entries_buf[offset + 44],
            entries_buf[offset + 45],
            entries_buf[offset + 46],
            entries_buf[offset + 47],
        ]);
        
        // Parse partition name (UTF-16LE, 36 characters max)
        let mut name_utf16 = Vec::new();
        for j in 0..36 {
            let char_offset = offset + 56 + (j * 2);
            let c = u16::from_le_bytes([
                entries_buf[char_offset],
                entries_buf[char_offset + 1],
            ]);
            
            if c == 0 {
                break;
            }
            name_utf16.push(c);
        }
        
        let name = String::from_utf16_lossy(&name_utf16);
        
        // Determine partition type from GUID
        let type_guid = &entries_buf[offset..offset + 16];
        let partition_type = guess_partition_type_from_guid(type_guid);
        
        partitions.push(Partition {
            number: i,
            partition_type,
            start_lba: starting_lba,
            size_sectors: ending_lba - starting_lba + 1,
            name: Some(name),
            bootable: false, // GPT doesn't have bootable flag
        });
    }
    
    Ok(partitions)
}

/// Guess partition type from GPT type GUID
fn guess_partition_type_from_guid(guid: &[u8]) -> PartitionType {
    // Common GUIDs (first few bytes)
    match &guid[0..4] {
        // EFI System Partition
        [0x28, 0x73, 0x2A, 0xC1] => PartitionType::Efi,
        
        // Microsoft Basic Data (FAT/NTFS)
        [0xA2, 0xA0, 0xD0, 0xEB] => PartitionType::Fat32, // Assume FAT32
        
        // Linux filesystem
        [0xAF, 0x3D, 0xC6, 0x0F] => PartitionType::Ext,
        
        // Linux swap
        [0x57, 0x65, 0x82, 0x06] => PartitionType::LinuxSwap,
        
        _ => PartitionType::Unknown(0xFF),
    }
}

/// Detect partition table type
pub fn detect_partition_table_type<D: BlockDevice>(device: &mut D) -> DriverResult<PartitionTableType> {
    let mut buffer = vec![0u8; 512];
    device.read(0, &mut buffer)?;
    
    // Check MBR signature
    if buffer[510] != 0x55 || buffer[511] != 0xAA {
        return Ok(PartitionTableType::Unknown);
    }
    
    // Check for GPT
    if is_gpt_protective_mbr(&buffer) {
        Ok(PartitionTableType::Gpt)
    } else {
        Ok(PartitionTableType::Mbr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_partition_type_from_u8() {
        assert_eq!(PartitionType::from(0x00), PartitionType::Empty);
        assert_eq!(PartitionType::from(0x0B), PartitionType::Fat32);
        assert_eq!(PartitionType::from(0x83), PartitionType::Ext);
        assert_eq!(PartitionType::from(0xEF), PartitionType::Efi);
    }
    
    #[test]
    fn test_partition_size_bytes() {
        let partition = Partition {
            number: 0,
            partition_type: PartitionType::Fat32,
            start_lba: 2048,
            size_sectors: 1024,
            name: None,
            bootable: false,
        };
        
        assert_eq!(partition.size_bytes(), 1024 * 512);
    }
    
    #[test]
    fn test_partition_is_empty() {
        let empty = Partition {
            number: 0,
            partition_type: PartitionType::Empty,
            start_lba: 0,
            size_sectors: 0,
            name: None,
            bootable: false,
        };
        
        assert!(empty.is_empty());
    }
}
