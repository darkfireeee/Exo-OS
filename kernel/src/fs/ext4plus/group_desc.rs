//! Block Group Descriptors
//!
//! Manages block group descriptors for ext4plus filesystem.
//! Each block group has a descriptor that tracks:
//! - Block bitmap location
//! - Inode bitmap location
//! - Inode table location
//! - Free blocks/inodes count
//! - Used directories count
//! - Checksums and flags

use crate::fs::{FsError, FsResult};
use crate::fs::block::BlockDevice;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use super::superblock::Ext4plusSuperblock;

/// Block Group Descriptor (ext4 64-bit version)
#[derive(Debug, Clone, Copy)]
pub struct GroupDescriptor {
    /// Block bitmap block (low 32 bits)
    pub block_bitmap_lo: u32,
    /// Inode bitmap block (low 32 bits)
    pub inode_bitmap_lo: u32,
    /// Inode table block (low 32 bits)
    pub inode_table_lo: u32,
    /// Free blocks count (low 16 bits)
    pub free_blocks_count_lo: u16,
    /// Free inodes count (low 16 bits)
    pub free_inodes_count_lo: u16,
    /// Used directories count (low 16 bits)
    pub used_dirs_count_lo: u16,
    /// Flags
    pub flags: u16,
    /// Exclude bitmap (low 32 bits)
    pub exclude_bitmap_lo: u32,
    /// Block bitmap checksum (low 16 bits)
    pub block_bitmap_csum_lo: u16,
    /// Inode bitmap checksum (low 16 bits)
    pub inode_bitmap_csum_lo: u16,
    /// Unused inode count (low 16 bits)
    pub itable_unused_lo: u16,
    /// Group descriptor checksum
    pub checksum: u16,

    /// Block bitmap block (high 32 bits)
    pub block_bitmap_hi: u32,
    /// Inode bitmap block (high 32 bits)
    pub inode_bitmap_hi: u32,
    /// Inode table block (high 32 bits)
    pub inode_table_hi: u32,
    /// Free blocks count (high 16 bits)
    pub free_blocks_count_hi: u16,
    /// Free inodes count (high 16 bits)
    pub free_inodes_count_hi: u16,
    /// Used directories count (high 16 bits)
    pub used_dirs_count_hi: u16,
    /// Unused inode count (high 16 bits)
    pub itable_unused_hi: u16,
    /// Exclude bitmap (high 32 bits)
    pub exclude_bitmap_hi: u32,
    /// Block bitmap checksum (high 16 bits)
    pub block_bitmap_csum_hi: u16,
    /// Inode bitmap checksum (high 16 bits)
    pub inode_bitmap_csum_hi: u16,
    /// Reserved
    pub reserved: u32,
}

impl GroupDescriptor {
    /// Get block bitmap block number (64-bit)
    pub fn block_bitmap(&self) -> u64 {
        ((self.block_bitmap_hi as u64) << 32) | (self.block_bitmap_lo as u64)
    }

    /// Get inode bitmap block number (64-bit)
    pub fn inode_bitmap(&self) -> u64 {
        ((self.inode_bitmap_hi as u64) << 32) | (self.inode_bitmap_lo as u64)
    }

    /// Get inode table block number (64-bit)
    pub fn inode_table(&self) -> u64 {
        ((self.inode_table_hi as u64) << 32) | (self.inode_table_lo as u64)
    }

    /// Get free blocks count (32-bit)
    pub fn free_blocks_count(&self) -> u32 {
        ((self.free_blocks_count_hi as u32) << 16) | (self.free_blocks_count_lo as u32)
    }

    /// Get free inodes count (32-bit)
    pub fn free_inodes_count(&self) -> u32 {
        ((self.free_inodes_count_hi as u32) << 16) | (self.free_inodes_count_lo as u32)
    }

    /// Get used directories count (32-bit)
    pub fn used_dirs_count(&self) -> u32 {
        ((self.used_dirs_count_hi as u32) << 16) | (self.used_dirs_count_lo as u32)
    }

    /// Set free blocks count (32-bit)
    pub fn set_free_blocks_count(&mut self, count: u32) {
        self.free_blocks_count_lo = count as u16;
        self.free_blocks_count_hi = (count >> 16) as u16;
    }

    /// Set free inodes count (32-bit)
    pub fn set_free_inodes_count(&mut self, count: u32) {
        self.free_inodes_count_lo = count as u16;
        self.free_inodes_count_hi = (count >> 16) as u16;
    }

    /// Check if group is marked as uninitialized
    pub fn is_uninitialized(&self) -> bool {
        (self.flags & 0x01) != 0
    }

    /// Check if block bitmap is uninitialized
    pub fn is_block_bitmap_uninitialized(&self) -> bool {
        (self.flags & 0x02) != 0
    }

    /// Check if inode table is uninitialized
    pub fn is_inode_table_uninitialized(&self) -> bool {
        (self.flags & 0x04) != 0
    }

    /// Parse from raw bytes
    pub fn parse(data: &[u8]) -> FsResult<Self> {
        if data.len() < 64 {
            return Err(FsError::InvalidData);
        }

        Ok(Self {
            block_bitmap_lo: u32::from_le_bytes([data[0], data[1], data[2], data[3]]),
            inode_bitmap_lo: u32::from_le_bytes([data[4], data[5], data[6], data[7]]),
            inode_table_lo: u32::from_le_bytes([data[8], data[9], data[10], data[11]]),
            free_blocks_count_lo: u16::from_le_bytes([data[12], data[13]]),
            free_inodes_count_lo: u16::from_le_bytes([data[14], data[15]]),
            used_dirs_count_lo: u16::from_le_bytes([data[16], data[17]]),
            flags: u16::from_le_bytes([data[18], data[19]]),
            exclude_bitmap_lo: u32::from_le_bytes([data[20], data[21], data[22], data[23]]),
            block_bitmap_csum_lo: u16::from_le_bytes([data[24], data[25]]),
            inode_bitmap_csum_lo: u16::from_le_bytes([data[26], data[27]]),
            itable_unused_lo: u16::from_le_bytes([data[28], data[29]]),
            checksum: u16::from_le_bytes([data[30], data[31]]),
            block_bitmap_hi: u32::from_le_bytes([data[32], data[33], data[34], data[35]]),
            inode_bitmap_hi: u32::from_le_bytes([data[36], data[37], data[38], data[39]]),
            inode_table_hi: u32::from_le_bytes([data[40], data[41], data[42], data[43]]),
            free_blocks_count_hi: u16::from_le_bytes([data[44], data[45]]),
            free_inodes_count_hi: u16::from_le_bytes([data[46], data[47]]),
            used_dirs_count_hi: u16::from_le_bytes([data[48], data[49]]),
            itable_unused_hi: u16::from_le_bytes([data[50], data[51]]),
            exclude_bitmap_hi: u32::from_le_bytes([data[52], data[53], data[54], data[55]]),
            block_bitmap_csum_hi: u16::from_le_bytes([data[56], data[57]]),
            inode_bitmap_csum_hi: u16::from_le_bytes([data[58], data[59]]),
            reserved: u32::from_le_bytes([data[60], data[61], data[62], data[63]]),
        })
    }

    /// Serialize to raw bytes
    pub fn serialize(&self, data: &mut [u8]) {
        data[0..4].copy_from_slice(&self.block_bitmap_lo.to_le_bytes());
        data[4..8].copy_from_slice(&self.inode_bitmap_lo.to_le_bytes());
        data[8..12].copy_from_slice(&self.inode_table_lo.to_le_bytes());
        data[12..14].copy_from_slice(&self.free_blocks_count_lo.to_le_bytes());
        data[14..16].copy_from_slice(&self.free_inodes_count_lo.to_le_bytes());
        data[16..18].copy_from_slice(&self.used_dirs_count_lo.to_le_bytes());
        data[18..20].copy_from_slice(&self.flags.to_le_bytes());
        data[20..24].copy_from_slice(&self.exclude_bitmap_lo.to_le_bytes());
        data[24..26].copy_from_slice(&self.block_bitmap_csum_lo.to_le_bytes());
        data[26..28].copy_from_slice(&self.inode_bitmap_csum_lo.to_le_bytes());
        data[28..30].copy_from_slice(&self.itable_unused_lo.to_le_bytes());
        data[30..32].copy_from_slice(&self.checksum.to_le_bytes());
        data[32..36].copy_from_slice(&self.block_bitmap_hi.to_le_bytes());
        data[36..40].copy_from_slice(&self.inode_bitmap_hi.to_le_bytes());
        data[40..44].copy_from_slice(&self.inode_table_hi.to_le_bytes());
        data[44..46].copy_from_slice(&self.free_blocks_count_hi.to_le_bytes());
        data[46..48].copy_from_slice(&self.free_inodes_count_hi.to_le_bytes());
        data[48..50].copy_from_slice(&self.used_dirs_count_hi.to_le_bytes());
        data[50..52].copy_from_slice(&self.itable_unused_hi.to_le_bytes());
        data[52..56].copy_from_slice(&self.exclude_bitmap_hi.to_le_bytes());
        data[56..58].copy_from_slice(&self.block_bitmap_csum_hi.to_le_bytes());
        data[58..60].copy_from_slice(&self.inode_bitmap_csum_hi.to_le_bytes());
        data[60..64].copy_from_slice(&self.reserved.to_le_bytes());
    }
}

/// Group Descriptor Table
pub struct GroupDescriptorTable {
    /// Array of group descriptors
    descriptors: Vec<GroupDescriptor>,
    /// Descriptor size (usually 32 or 64 bytes)
    desc_size: usize,
}

impl GroupDescriptorTable {
    /// Read group descriptor table from device
    pub fn read(device: &Arc<Mutex<dyn BlockDevice>>, sb: &Ext4plusSuperblock) -> FsResult<Self> {
        let groups_count = sb.block_groups_count();
        let desc_size = if sb.incompat_features().is_64bit { 64 } else { 32 };

        log::debug!("ext4plus: Reading {} group descriptors (size: {} bytes)", groups_count, desc_size);

        // Group descriptors start right after the superblock
        let gdt_block = if sb.block_size() == 1024 { 2 } else { 1 };
        let gdt_blocks = ((groups_count as usize * desc_size + sb.block_size() - 1) / sb.block_size()) as u64;

        let mut descriptors = Vec::with_capacity(groups_count as usize);
        let mut buffer = alloc::vec![0u8; sb.block_size() * gdt_blocks as usize];

        // Read GDT blocks
        {
            let dev = device.lock();
            for i in 0..gdt_blocks {
                let offset = i * sb.block_size() as u64;
                dev.read_blocks(
                    (gdt_block + i) * sb.block_size() as u64 / 512,
                    &mut buffer[offset as usize..(offset + sb.block_size() as u64) as usize],
                )?;
            }
        }

        // Parse descriptors
        for i in 0..groups_count as usize {
            let offset = i * desc_size;
            let desc = GroupDescriptor::parse(&buffer[offset..offset + desc_size])?;
            descriptors.push(desc);
        }

        log::debug!("ext4plus: Loaded {} group descriptors", descriptors.len());

        Ok(Self {
            descriptors,
            desc_size,
        })
    }

    /// Write group descriptor table to device
    pub fn write(&self, device: &Arc<Mutex<dyn BlockDevice>>, sb: &Ext4plusSuperblock) -> FsResult<()> {
        let gdt_block = if sb.block_size() == 1024 { 2 } else { 1 };
        let gdt_blocks = ((self.descriptors.len() * self.desc_size + sb.block_size() - 1) / sb.block_size()) as u64;

        let mut buffer = alloc::vec![0u8; sb.block_size() * gdt_blocks as usize];

        // Serialize descriptors
        for (i, desc) in self.descriptors.iter().enumerate() {
            let offset = i * self.desc_size;
            desc.serialize(&mut buffer[offset..offset + self.desc_size]);
        }

        // Write GDT blocks
        {
            let mut dev = device.lock();
            for i in 0..gdt_blocks {
                let offset = i * sb.block_size() as u64;
                dev.write_blocks(
                    (gdt_block + i) * sb.block_size() as u64 / 512,
                    &buffer[offset as usize..(offset + sb.block_size() as u64) as usize],
                )?;
            }
        }

        log::debug!("ext4plus: Wrote {} group descriptors", self.descriptors.len());
        Ok(())
    }

    /// Get descriptor for a group
    pub fn get(&self, group: u32) -> Option<&GroupDescriptor> {
        self.descriptors.get(group as usize)
    }

    /// Get mutable descriptor for a group
    pub fn get_mut(&mut self, group: u32) -> Option<&mut GroupDescriptor> {
        self.descriptors.get_mut(group as usize)
    }

    /// Get number of groups
    pub fn count(&self) -> u32 {
        self.descriptors.len() as u32
    }

    /// Iterator over all descriptors
    pub fn iter(&self) -> impl Iterator<Item = &GroupDescriptor> {
        self.descriptors.iter()
    }
}
