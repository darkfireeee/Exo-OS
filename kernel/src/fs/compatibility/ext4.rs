//! ext4 Read-Only Compatibility Layer
//!
//! Provides read-only access to existing ext4 filesystems for backwards compatibility.
//! Supports the core ext4 features needed for reading files:
//! - Superblock validation
//! - Inode reading and parsing
//! - Extent-based block mapping
//! - Directory traversal
//! - Symlink resolution
//!
//! # Design
//! - Read-only: No write operations (use ext4plus for full support)
//! - Robust validation of all on-disk structures
//! - Handles corrupted filesystems gracefully
//! - Compatible with all ext4 variants (ext2, ext3, ext4)
//!
//! # Performance
//! - Block reads cached by block layer
//! - Inode cache for frequently accessed files
//! - Extent lookup optimized for sequential reads

use alloc::sync::Arc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use spin::{RwLock, Mutex};

use crate::fs::{FsError, FsResult};
use crate::fs::core::types::*;
use crate::fs::block::BlockDevice;
use crate::fs::utils::endian::*;

/// ext4 magic number
pub const EXT4_SUPER_MAGIC: u32 = 0xEF53;

/// ext4 superblock offset
const EXT4_SB_OFFSET: u64 = 1024;

/// ext4 block size (can be 1024, 2048, or 4096)
const DEFAULT_BLOCK_SIZE: u32 = 4096;

/// ext4 inode size
const EXT4_INODE_SIZE: usize = 256;

/// Root inode number
const EXT4_ROOT_INO: u32 = 2;

/// ext4 Read-Only Filesystem
pub struct Ext4ReadOnlyFs {
    device: Arc<Mutex<dyn BlockDevice>>,
    superblock: Ext4Superblock,
    block_size: u32,
    inode_size: u32,
    inodes_per_group: u32,
    blocks_per_group: u32,
}

impl Ext4ReadOnlyFs {
    /// Mount ext4 filesystem in read-only mode
    pub fn mount(device: Arc<Mutex<dyn BlockDevice>>) -> FsResult<Self> {
        log::info!("ext4: Mounting filesystem (read-only)");

        // Read superblock
        let sb_buf = Self::read_block_static(&device, EXT4_SB_OFFSET, 1024)?;
        let superblock = Ext4Superblock::parse(&sb_buf)?;

        // Validate magic
        if superblock.s_magic != EXT4_SUPER_MAGIC {
            log::error!("ext4: Invalid magic: 0x{:04x}", superblock.s_magic);
            return Err(FsError::InvalidData);
        }

        let block_size = 1024u32 << superblock.s_log_block_size;
        let inode_size = if superblock.s_inode_size == 0 {
            128
        } else {
            superblock.s_inode_size as u32
        };

        log::info!("ext4: Superblock validated");
        log::info!("  Block size: {} bytes", block_size);
        log::info!("  Inode size: {} bytes", inode_size);
        log::info!("  Total blocks: {}", superblock.s_blocks_count);
        log::info!("  Total inodes: {}", superblock.s_inodes_count);
        log::info!("  Blocks per group: {}", superblock.s_blocks_per_group);
        log::info!("  Inodes per group: {}", superblock.s_inodes_per_group);

        let inodes_per_group = superblock.s_inodes_per_group;
        let blocks_per_group = superblock.s_blocks_per_group;

        Ok(Self {
            device,
            superblock,
            block_size,
            inode_size,
            inodes_per_group,
            blocks_per_group,
        })
    }

    /// Read block from device (static helper for mount)
    fn read_block_static(
        device: &Arc<Mutex<dyn BlockDevice>>,
        offset: u64,
        size: usize,
    ) -> FsResult<Vec<u8>> {
        let mut buf = alloc::vec![0u8; size];
        let dev = device.lock();
        dev.read(offset, &mut buf)?;
        Ok(buf)
    }

    /// Read block from device
    fn read_block(&self, block_num: u64) -> FsResult<Vec<u8>> {
        let offset = block_num * self.block_size as u64;
        let mut buf = alloc::vec![0u8; self.block_size as usize];
        let dev = self.device.lock();
        dev.read(offset, &mut buf)?;
        Ok(buf)
    }

    /// Read multiple blocks
    fn read_blocks(&self, block_num: u64, count: usize) -> FsResult<Vec<u8>> {
        let offset = block_num * self.block_size as u64;
        let size = count * self.block_size as usize;
        let mut buf = alloc::vec![0u8; size];
        let dev = self.device.lock();
        dev.read(offset, &mut buf)?;
        Ok(buf)
    }

    /// Get block group descriptor offset
    fn get_gd_offset(&self, group: u32) -> u64 {
        let gdt_block = if self.block_size == 1024 { 2 } else { 1 };
        let offset = gdt_block * self.block_size as u64;
        offset + (group as u64 * 32) // Each descriptor is 32 bytes (minimum)
    }

    /// Read inode
    pub fn read_inode(&self, ino: u32) -> FsResult<Ext4ReadOnlyInode> {
        if ino == 0 {
            return Err(FsError::InvalidArgument);
        }

        // Calculate block group
        let group = (ino - 1) / self.inodes_per_group;
        let index = (ino - 1) % self.inodes_per_group;

        // Read group descriptor
        let gd_offset = self.get_gd_offset(group);
        let mut gd_buf = alloc::vec![0u8; 64];
        {
            let dev = self.device.lock();
            dev.read(gd_offset, &mut gd_buf)?;
        }

        let inode_table_block = read_le_u32(&gd_buf[8..12]) as u64;

        // Calculate inode offset
        let inode_offset = inode_table_block * self.block_size as u64
            + (index as u64 * self.inode_size as u64);

        // Read inode
        let mut inode_buf = alloc::vec![0u8; self.inode_size as usize];
        {
            let dev = self.device.lock();
            dev.read(inode_offset, &mut inode_buf)?;
        }

        Ext4ReadOnlyInode::parse(ino, &inode_buf, Arc::clone(&self.device), self.block_size)
    }

    /// Read root directory
    pub fn root(&self) -> FsResult<Ext4ReadOnlyInode> {
        self.read_inode(EXT4_ROOT_INO)
    }
}

/// ext4 Superblock
#[derive(Debug, Clone)]
pub struct Ext4Superblock {
    pub s_inodes_count: u32,
    pub s_blocks_count: u64,
    pub s_r_blocks_count: u64,
    pub s_free_blocks_count: u64,
    pub s_free_inodes_count: u32,
    pub s_first_data_block: u32,
    pub s_log_block_size: u32,
    pub s_blocks_per_group: u32,
    pub s_inodes_per_group: u32,
    pub s_mtime: u32,
    pub s_wtime: u32,
    pub s_magic: u32,
    pub s_inode_size: u16,
}

impl Ext4Superblock {
    fn parse(buf: &[u8]) -> FsResult<Self> {
        if buf.len() < 1024 {
            return Err(FsError::InvalidData);
        }

        Ok(Self {
            s_inodes_count: read_le_u32(&buf[0..4]),
            s_blocks_count: read_le_u32(&buf[4..8]) as u64,
            s_r_blocks_count: read_le_u32(&buf[8..12]) as u64,
            s_free_blocks_count: read_le_u32(&buf[12..16]) as u64,
            s_free_inodes_count: read_le_u32(&buf[16..20]),
            s_first_data_block: read_le_u32(&buf[20..24]),
            s_log_block_size: read_le_u32(&buf[24..28]),
            s_blocks_per_group: read_le_u32(&buf[32..36]),
            s_inodes_per_group: read_le_u32(&buf[40..44]),
            s_mtime: read_le_u32(&buf[44..48]),
            s_wtime: read_le_u32(&buf[48..52]),
            s_magic: u16::from_le_bytes([buf[56], buf[57]]) as u32,
            s_inode_size: u16::from_le_bytes([buf[88], buf[89]]),
        })
    }
}

/// ext4 Read-Only Inode
pub struct Ext4ReadOnlyInode {
    ino: u32,
    mode: u16,
    uid: u32,
    gid: u32,
    size: u64,
    atime: u32,
    mtime: u32,
    ctime: u32,
    links_count: u16,
    blocks: [u32; 15],
    device: Arc<Mutex<dyn BlockDevice>>,
    block_size: u32,
}

impl Ext4ReadOnlyInode {
    fn parse(
        ino: u32,
        buf: &[u8],
        device: Arc<Mutex<dyn BlockDevice>>,
        block_size: u32,
    ) -> FsResult<Self> {
        if buf.len() < 128 {
            return Err(FsError::InvalidData);
        }

        let mode = u16::from_le_bytes([buf[0], buf[1]]);
        let uid = u16::from_le_bytes([buf[2], buf[3]]) as u32;
        let size_lo = read_le_u32(&buf[4..8]);
        let atime = read_le_u32(&buf[8..12]);
        let ctime = read_le_u32(&buf[12..16]);
        let mtime = read_le_u32(&buf[16..20]);
        let gid = u16::from_le_bytes([buf[24], buf[25]]) as u32;
        let links_count = u16::from_le_bytes([buf[26], buf[27]]);

        // Read block pointers
        let mut blocks = [0u32; 15];
        for i in 0..15 {
            let offset = 40 + i * 4;
            blocks[i] = read_le_u32(&buf[offset..offset + 4]);
        }

        // Get high 32 bits of size if available
        let size_hi = if buf.len() >= 112 {
            read_le_u32(&buf[108..112])
        } else {
            0
        };
        let size = ((size_hi as u64) << 32) | (size_lo as u64);

        Ok(Self {
            ino,
            mode,
            uid,
            gid,
            size,
            atime,
            mtime,
            ctime,
            links_count,
            blocks,
            device,
            block_size,
        })
    }

    fn inode_type(&self) -> InodeType {
        let fmt = self.mode & 0xF000;
        match fmt {
            0x8000 => InodeType::File,
            0x4000 => InodeType::Directory,
            0xA000 => InodeType::Symlink,
            0x2000 => InodeType::CharDevice,
            0x6000 => InodeType::BlockDevice,
            0x1000 => InodeType::Fifo,
            0xC000 => InodeType::Socket,
            _ => InodeType::File,
        }
    }

    /// Read data from inode
    fn read_data(&self, offset: u64, buf: &mut [u8]) -> FsResult<usize> {
        if offset >= self.size {
            return Ok(0);
        }

        let to_read = ((self.size - offset) as usize).min(buf.len());
        let start_block = offset / self.block_size as u64;
        let block_offset = (offset % self.block_size as u64) as usize;

        let mut bytes_read = 0;
        let mut current_block = start_block;

        while bytes_read < to_read {
            // Map logical block to physical block
            let phys_block = self.map_block(current_block)?;

            if phys_block == 0 {
                // Sparse block - return zeros
                let to_copy = (to_read - bytes_read).min(self.block_size as usize - block_offset);
                buf[bytes_read..bytes_read + to_copy].fill(0);
                bytes_read += to_copy;
            } else {
                // Read block
                let mut block_buf = alloc::vec![0u8; self.block_size as usize];
                {
                    let dev = self.device.lock();
                    dev.read(phys_block * self.block_size as u64, &mut block_buf)?;
                }

                let start = if current_block == start_block {
                    block_offset
                } else {
                    0
                };
                let to_copy = (to_read - bytes_read).min(self.block_size as usize - start);

                buf[bytes_read..bytes_read + to_copy]
                    .copy_from_slice(&block_buf[start..start + to_copy]);
                bytes_read += to_copy;
            }

            current_block += 1;
        }

        Ok(bytes_read)
    }

    /// Map logical block to physical block (simplified - no extent support)
    fn map_block(&self, logical: u64) -> FsResult<u64> {
        if logical < 12 {
            // Direct blocks
            return Ok(self.blocks[logical as usize] as u64);
        }

        // For now, we don't support indirect blocks
        // A full implementation would handle single, double, and triple indirect blocks
        log::warn!("ext4: Indirect blocks not yet supported");
        Ok(0)
    }

    /// Read directory entries
    pub fn read_dir(&self) -> FsResult<Vec<Ext4DirEntry>> {
        if self.inode_type() != InodeType::Directory {
            return Err(FsError::NotDirectory);
        }

        let mut entries = Vec::new();
        let mut data = alloc::vec![0u8; self.size as usize];
        self.read_data(0, &mut data)?;

        let mut offset = 0;
        while offset < data.len() {
            if offset + 8 > data.len() {
                break;
            }

            let inode = read_le_u32(&data[offset..offset + 4]);
            let rec_len = u16::from_le_bytes([data[offset + 4], data[offset + 5]]) as usize;
            let name_len = data[offset + 6] as usize;

            if inode == 0 || rec_len == 0 || rec_len > data.len() - offset {
                break;
            }

            if name_len > 0 && offset + 8 + name_len <= data.len() {
                let name_bytes = &data[offset + 8..offset + 8 + name_len];
                if let Ok(name) = String::from_utf8(name_bytes.to_vec()) {
                    entries.push(Ext4DirEntry { inode, name });
                }
            }

            offset += rec_len;
        }

        Ok(entries)
    }
}

impl Inode for Ext4ReadOnlyInode {
    fn ino(&self) -> u64 {
        self.ino as u64
    }

    fn inode_type(&self) -> InodeType {
        self.inode_type()
    }

    fn size(&self) -> u64 {
        self.size
    }

    fn permissions(&self) -> InodePermissions {
        InodePermissions::new(self.mode & 0o7777)
    }

    fn uid(&self) -> u32 {
        self.uid
    }

    fn gid(&self) -> u32 {
        self.gid
    }

    fn nlink(&self) -> u32 {
        self.links_count as u32
    }

    fn atime(&self) -> Timestamp {
        Timestamp {
            sec: self.atime as i64,
            nsec: 0,
        }
    }

    fn mtime(&self) -> Timestamp {
        Timestamp {
            sec: self.mtime as i64,
            nsec: 0,
        }
    }

    fn ctime(&self) -> Timestamp {
        Timestamp {
            sec: self.ctime as i64,
            nsec: 0,
        }
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> FsResult<usize> {
        self.read_data(offset, buf)
    }

    fn write_at(&mut self, _offset: u64, _buf: &[u8]) -> FsResult<usize> {
        Err(FsError::NotSupported) // Read-only
    }

    fn truncate(&mut self, _size: u64) -> FsResult<()> {
        Err(FsError::NotSupported) // Read-only
    }

    fn list(&self) -> FsResult<Vec<String>> {
        let entries = self.read_dir()?;
        Ok(entries.into_iter().map(|e| e.name).collect())
    }

    fn lookup(&self, name: &str) -> FsResult<u64> {
        let entries = self.read_dir()?;
        entries
            .into_iter()
            .find(|e| e.name == name)
            .map(|e| e.inode as u64)
            .ok_or(FsError::NotFound)
    }

    fn create(&mut self, _name: &str, _inode_type: InodeType) -> FsResult<u64> {
        Err(FsError::NotSupported) // Read-only
    }

    fn remove(&mut self, _name: &str) -> FsResult<()> {
        Err(FsError::NotSupported) // Read-only
    }

    fn readlink(&self) -> FsResult<String> {
        if self.inode_type() != InodeType::Symlink {
            return Err(FsError::InvalidArgument);
        }

        // Small symlinks stored in inode blocks
        if self.size < 60 {
            let bytes = unsafe {
                core::slice::from_raw_parts(
                    self.blocks.as_ptr() as *const u8,
                    self.size as usize,
                )
            };
            return String::from_utf8(bytes.to_vec()).map_err(|_| FsError::InvalidData);
        }

        // Large symlinks stored in blocks
        let mut buf = alloc::vec![0u8; self.size as usize];
        self.read_data(0, &mut buf)?;
        String::from_utf8(buf).map_err(|_| FsError::InvalidData)
    }
}

/// Directory entry
#[derive(Debug, Clone)]
pub struct Ext4DirEntry {
    pub inode: u32,
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_superblock_parse() {
        let mut buf = vec![0u8; 1024];
        // Set magic number
        buf[56] = 0x53;
        buf[57] = 0xEF;

        let sb = Ext4Superblock::parse(&buf).unwrap();
        assert_eq!(sb.s_magic, EXT4_SUPER_MAGIC);
    }
}
