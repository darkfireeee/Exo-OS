//! Partition Management
//!
//! Automatic detection and parsing of MBR and GPT partition tables.

use alloc::sync::Arc;
use alloc::string::String;
use alloc::vec::Vec;
use spin::RwLock;

use crate::fs::{FsError, FsResult};
use super::device::{BlockDevice, BlockDeviceInfo, DeviceType};

/// Partition information
#[derive(Debug, Clone)]
pub struct Partition {
    /// Partition number (1-based)
    pub number: u8,
    /// Start LBA (Logical Block Address)
    pub start_lba: u64,
    /// Size in blocks
    pub size_blocks: u64,
    /// Partition type
    pub partition_type: PartitionType,
    /// Partition flags
    pub flags: PartitionFlags,
    /// File system type (if detected)
    pub fs_type: Option<FileSystemType>,
    /// Partition label/name
    pub label: String,
}

impl Partition {
    /// Get partition size in bytes
    pub fn size_bytes(&self, block_size: u32) -> u64 {
        self.size_blocks * block_size as u64
    }

    /// Get start offset in bytes
    pub fn start_offset(&self, block_size: u32) -> u64 {
        self.start_lba * block_size as u64
    }

    /// Check if partition is bootable
    pub fn is_bootable(&self) -> bool {
        self.flags.bootable
    }
}

/// Partition type (MBR or GPT)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionType {
    /// Empty/unused partition
    Empty,
    /// FAT12/FAT16
    FAT16,
    /// FAT32
    FAT32,
    /// NTFS
    NTFS,
    /// Linux native (ext2/ext3/ext4)
    Linux,
    /// Linux swap
    LinuxSwap,
    /// Extended partition
    Extended,
    /// EFI System Partition
    EFI,
    /// Unknown type
    Unknown(u8),
}

impl PartitionType {
    /// Create from MBR partition type byte
    pub fn from_mbr_type(type_byte: u8) -> Self {
        match type_byte {
            0x00 => Self::Empty,
            0x01 | 0x04 | 0x06 => Self::FAT16,
            0x0B | 0x0C => Self::FAT32,
            0x07 => Self::NTFS,
            0x83 => Self::Linux,
            0x82 => Self::LinuxSwap,
            0x05 | 0x0F => Self::Extended,
            0xEF => Self::EFI,
            _ => Self::Unknown(type_byte),
        }
    }
}

/// Partition flags
#[derive(Debug, Clone, Copy, Default)]
pub struct PartitionFlags {
    /// Bootable/active flag
    pub bootable: bool,
    /// Read-only flag
    pub read_only: bool,
    /// Hidden flag
    pub hidden: bool,
}

/// Detected filesystem type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSystemType {
    FAT12,
    FAT16,
    FAT32,
    NTFS,
    Ext2,
    Ext3,
    Ext4,
    XFS,
    Btrfs,
    ZFS,
    Unknown,
}

/// Partition table type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionTableType {
    MBR,
    GPT,
    Unknown,
}

/// Partition table
pub struct PartitionTable {
    /// Table type
    pub table_type: PartitionTableType,
    /// List of partitions
    pub partitions: Vec<Partition>,
    /// Disk signature (for MBR)
    pub disk_signature: u32,
}

impl PartitionTable {
    /// Detect and parse partition table from device
    pub fn detect(device: &dyn BlockDevice) -> FsResult<Self> {
        let mut buf = alloc::vec![0u8; 512];
        device.read(0, &mut buf)?;

        if Self::is_gpt(&buf) {
            Self::parse_gpt(device, buf)
        } else if Self::is_mbr(&buf) {
            Self::parse_mbr(buf)
        } else {
            Ok(Self::empty())
        }
    }

    /// Check if buffer contains GPT signature
    fn is_gpt(buf: &[u8]) -> bool {
        if buf.len() < 512 {
            return false;
        }

        &buf[0x1FE..0x200] == &[0x55, 0xAA] &&
        buf.len() >= 512 + 92 &&
        &buf[512..520] == b"EFI PART"
    }

    /// Check if buffer contains MBR signature
    fn is_mbr(buf: &[u8]) -> bool {
        buf.len() >= 512 && &buf[0x1FE..0x200] == &[0x55, 0xAA]
    }

    /// Parse MBR partition table
    fn parse_mbr(buf: Vec<u8>) -> FsResult<Self> {
        let disk_signature = u32::from_le_bytes([
            buf[0x1B8],
            buf[0x1B9],
            buf[0x1BA],
            buf[0x1BB],
        ]);

        let mut partitions = Vec::new();

        for i in 0..4 {
            let offset = 0x1BE + (i * 16);
            let entry = &buf[offset..offset + 16];

            let bootable = entry[0] == 0x80;
            let type_byte = entry[4];
            let start_lba = u32::from_le_bytes([
                entry[8],
                entry[9],
                entry[10],
                entry[11],
            ]) as u64;
            let size_blocks = u32::from_le_bytes([
                entry[12],
                entry[13],
                entry[14],
                entry[15],
            ]) as u64;

            if type_byte == 0 || size_blocks == 0 {
                continue;
            }

            partitions.push(Partition {
                number: (i as u8) + 1,
                start_lba,
                size_blocks,
                partition_type: PartitionType::from_mbr_type(type_byte),
                flags: PartitionFlags {
                    bootable,
                    read_only: false,
                    hidden: false,
                },
                fs_type: None,
                label: String::new(),
            });
        }

        Ok(Self {
            table_type: PartitionTableType::MBR,
            partitions,
            disk_signature,
        })
    }

    /// Parse GPT partition table
    fn parse_gpt(device: &dyn BlockDevice, _mbr: Vec<u8>) -> FsResult<Self> {
        let mut gpt_header = alloc::vec![0u8; 512];
        device.read(512, &mut gpt_header)?;

        if &gpt_header[0..8] != b"EFI PART" {
            return Err(FsError::InvalidData);
        }

        let num_entries = u32::from_le_bytes([
            gpt_header[80],
            gpt_header[81],
            gpt_header[82],
            gpt_header[83],
        ]);

        let entry_size = u32::from_le_bytes([
            gpt_header[84],
            gpt_header[85],
            gpt_header[86],
            gpt_header[87],
        ]);

        let entries_lba = u64::from_le_bytes([
            gpt_header[72],
            gpt_header[73],
            gpt_header[74],
            gpt_header[75],
            gpt_header[76],
            gpt_header[77],
            gpt_header[78],
            gpt_header[79],
        ]);

        let mut partitions = Vec::new();
        let entries_size = (num_entries * entry_size) as usize;
        let mut entries_buf = alloc::vec![0u8; entries_size];

        device.read(entries_lba * 512, &mut entries_buf)?;

        for i in 0..num_entries as usize {
            let offset = i * entry_size as usize;
            let entry = &entries_buf[offset..offset + entry_size as usize];

            let is_empty = entry[0..16].iter().all(|&b| b == 0);
            if is_empty {
                continue;
            }

            let start_lba = u64::from_le_bytes([
                entry[32], entry[33], entry[34], entry[35],
                entry[36], entry[37], entry[38], entry[39],
            ]);

            let end_lba = u64::from_le_bytes([
                entry[40], entry[41], entry[42], entry[43],
                entry[44], entry[45], entry[46], entry[47],
            ]);

            let size_blocks = end_lba.saturating_sub(start_lba) + 1;
            let flags_raw = u64::from_le_bytes([
                entry[48], entry[49], entry[50], entry[51],
                entry[52], entry[53], entry[54], entry[55],
            ]);

            partitions.push(Partition {
                number: (i as u8) + 1,
                start_lba,
                size_blocks,
                partition_type: PartitionType::Linux,
                flags: PartitionFlags {
                    bootable: (flags_raw & 0x2) != 0,
                    read_only: (flags_raw & 0x1000000000000000) != 0,
                    hidden: (flags_raw & 0x2000000000000000) != 0,
                },
                fs_type: None,
                label: String::new(),
            });
        }

        Ok(Self {
            table_type: PartitionTableType::GPT,
            partitions,
            disk_signature: 0,
        })
    }

    /// Create empty partition table
    fn empty() -> Self {
        Self {
            table_type: PartitionTableType::Unknown,
            partitions: Vec::new(),
            disk_signature: 0,
        }
    }

    /// Get partition by number
    pub fn get(&self, number: u8) -> Option<&Partition> {
        self.partitions.iter().find(|p| p.number == number)
    }

    /// Get all partitions
    pub fn all(&self) -> &[Partition] {
        &self.partitions
    }

    /// Count partitions
    pub fn count(&self) -> usize {
        self.partitions.len()
    }
}

/// Partitioned block device - Wraps a partition as a block device
pub struct PartitionedDevice {
    /// Underlying device
    device: Arc<RwLock<dyn BlockDevice>>,
    /// Partition info
    partition: Partition,
    /// Device info (synthesized)
    info: BlockDeviceInfo,
}

impl PartitionedDevice {
    /// Create a new partitioned device
    pub fn new(
        device: Arc<RwLock<dyn BlockDevice>>,
        partition: Partition,
    ) -> Arc<RwLock<Self>> {
        let device_guard = device.read();
        let base_info = device_guard.info();
        let block_size = base_info.block_size;

        let info = BlockDeviceInfo {
            name: alloc::format!("{}p{}", base_info.name, partition.number),
            size: partition.size_bytes(block_size),
            block_size,
            read_only: base_info.read_only || partition.flags.read_only,
            device_type: base_info.device_type,
            vendor: base_info.vendor.clone(),
            model: base_info.model.clone(),
            serial: alloc::format!("{}-p{}", base_info.serial, partition.number),
        };

        drop(device_guard);

        Arc::new(RwLock::new(Self {
            device,
            partition,
            info,
        }))
    }
}

impl BlockDevice for PartitionedDevice {
    fn read(&self, offset: u64, buf: &mut [u8]) -> FsResult<usize> {
        if !self.is_aligned(offset, buf.len()) {
            return Err(FsError::InvalidArgument);
        }

        let partition_start = self.partition.start_offset(self.info.block_size);
        let actual_offset = partition_start + offset;

        if offset + buf.len() as u64 > self.info.size {
            return Err(FsError::InvalidArgument);
        }

        self.device.read().read(actual_offset, buf)
    }

    fn write(&mut self, offset: u64, buf: &[u8]) -> FsResult<usize> {
        if !self.is_aligned(offset, buf.len()) {
            return Err(FsError::InvalidArgument);
        }

        if self.info.read_only {
            return Err(FsError::PermissionDenied);
        }

        let partition_start = self.partition.start_offset(self.info.block_size);
        let actual_offset = partition_start + offset;

        if offset + buf.len() as u64 > self.info.size {
            return Err(FsError::InvalidArgument);
        }

        self.device.write().write(actual_offset, buf)
    }

    fn flush(&mut self) -> FsResult<()> {
        self.device.write().flush()
    }

    fn info(&self) -> &BlockDeviceInfo {
        &self.info
    }

    fn discard(&mut self, offset: u64, len: u64) -> FsResult<()> {
        let partition_start = self.partition.start_offset(self.info.block_size);
        let actual_offset = partition_start + offset;
        self.device.write().discard(actual_offset, len)
    }
}
