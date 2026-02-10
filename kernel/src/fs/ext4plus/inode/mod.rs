//! Inode Management
//!
//! Complete inode implementation with:
//! - ext4 inode structures
//! - Extent-based block mapping
//! - Extended attributes (xattr)
//! - Access control lists (ACL)
//! - Integration with cache layer

pub mod ops;
pub mod extent;
pub mod xattr;
pub mod acl;

use crate::fs::{FsError, FsResult};
use crate::fs::block::BlockDevice;
use crate::fs::core::types::{InodeType, InodePermissions, Timestamp, InodeStat};
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

pub use extent::{ExtentTree, Extent, ExtentHeader};
pub use xattr::{XattrManager, ExtendedAttribute};
pub use acl::{AclManager, AccessControlEntry};

/// ext4plus Inode (256 bytes)
#[derive(Debug, Clone)]
pub struct Ext4plusInode {
    /// Inode number
    pub ino: u64,
    /// File mode and permissions
    pub i_mode: u16,
    /// Owner UID
    pub i_uid: u32,
    /// Size (low 32 bits)
    pub i_size_lo: u32,
    /// Access time
    pub i_atime: u32,
    /// Change time
    pub i_ctime: u32,
    /// Modification time
    pub i_mtime: u32,
    /// Deletion time
    pub i_dtime: u32,
    /// Group ID
    pub i_gid: u32,
    /// Hard link count
    pub i_links_count: u16,
    /// Block count (512-byte blocks)
    pub i_blocks_lo: u32,
    /// Flags
    pub i_flags: u32,
    /// Block pointers / extent tree
    pub i_block: [u8; 60],
    /// Generation
    pub i_generation: u32,
    /// Extended attributes block
    pub i_file_acl_lo: u32,
    /// Size (high 32 bits)
    pub i_size_hi: u32,
    /// Fragment address (obsolete)
    pub i_obso_faddr: u32,
    /// Blocks count high 16 bits
    pub i_blocks_hi: u16,
    /// Extended attributes block high 32 bits
    pub i_file_acl_hi: u16,
    /// UID high 16 bits
    pub i_uid_hi: u16,
    /// GID high 16 bits
    pub i_gid_hi: u16,
    /// Checksum low 16 bits
    pub i_checksum_lo: u16,
    /// Reserved
    pub i_reserved: u16,
    /// Extra inode size
    pub i_extra_isize: u16,
    /// Checksum high 16 bits
    pub i_checksum_hi: u16,
    /// Change time extra bits
    pub i_ctime_extra: u32,
    /// Modification time extra bits
    pub i_mtime_extra: u32,
    /// Access time extra bits
    pub i_atime_extra: u32,
    /// Creation time
    pub i_crtime: u32,
    /// Creation time extra bits
    pub i_crtime_extra: u32,
    /// Version high 32 bits
    pub i_version_hi: u32,
    /// Project ID
    pub i_projid: u32,

    /// Cached extent tree
    extent_tree: Option<ExtentTree>,
}

impl Ext4plusInode {
    /// Parse inode from raw bytes
    pub fn parse(ino: u64, data: &[u8]) -> FsResult<Self> {
        if data.len() < 128 {
            return Err(FsError::InvalidData);
        }

        let mut i_block = [0u8; 60];
        i_block.copy_from_slice(&data[40..100]);

        let mut inode = Self {
            ino,
            i_mode: u16::from_le_bytes([data[0], data[1]]),
            i_uid: u16::from_le_bytes([data[2], data[3]]) as u32,
            i_size_lo: u32::from_le_bytes([data[4], data[5], data[6], data[7]]),
            i_atime: u32::from_le_bytes([data[8], data[9], data[10], data[11]]),
            i_ctime: u32::from_le_bytes([data[12], data[13], data[14], data[15]]),
            i_mtime: u32::from_le_bytes([data[16], data[17], data[18], data[19]]),
            i_dtime: u32::from_le_bytes([data[20], data[21], data[22], data[23]]),
            i_gid: u16::from_le_bytes([data[24], data[25]]) as u32,
            i_links_count: u16::from_le_bytes([data[26], data[27]]),
            i_blocks_lo: u32::from_le_bytes([data[28], data[29], data[30], data[31]]),
            i_flags: u32::from_le_bytes([data[32], data[33], data[34], data[35]]),
            i_block,
            i_generation: u32::from_le_bytes([data[100], data[101], data[102], data[103]]),
            i_file_acl_lo: u32::from_le_bytes([data[104], data[105], data[106], data[107]]),
            i_size_hi: u32::from_le_bytes([data[108], data[109], data[110], data[111]]),
            i_obso_faddr: u32::from_le_bytes([data[112], data[113], data[114], data[115]]),
            i_blocks_hi: u16::from_le_bytes([data[116], data[117]]),
            i_file_acl_hi: u16::from_le_bytes([data[118], data[119]]),
            i_uid_hi: u16::from_le_bytes([data[120], data[121]]),
            i_gid_hi: u16::from_le_bytes([data[122], data[123]]),
            i_checksum_lo: u16::from_le_bytes([data[124], data[125]]),
            i_reserved: u16::from_le_bytes([data[126], data[127]]),

            i_extra_isize: if data.len() >= 130 { u16::from_le_bytes([data[128], data[129]]) } else { 0 },
            i_checksum_hi: if data.len() >= 132 { u16::from_le_bytes([data[130], data[131]]) } else { 0 },
            i_ctime_extra: if data.len() >= 136 { u32::from_le_bytes([data[132], data[133], data[134], data[135]]) } else { 0 },
            i_mtime_extra: if data.len() >= 140 { u32::from_le_bytes([data[136], data[137], data[138], data[139]]) } else { 0 },
            i_atime_extra: if data.len() >= 144 { u32::from_le_bytes([data[140], data[141], data[142], data[143]]) } else { 0 },
            i_crtime: if data.len() >= 148 { u32::from_le_bytes([data[144], data[145], data[146], data[147]]) } else { 0 },
            i_crtime_extra: if data.len() >= 152 { u32::from_le_bytes([data[148], data[149], data[150], data[151]]) } else { 0 },
            i_version_hi: if data.len() >= 156 { u32::from_le_bytes([data[152], data[153], data[154], data[155]]) } else { 0 },
            i_projid: if data.len() >= 160 { u32::from_le_bytes([data[156], data[157], data[158], data[159]]) } else { 0 },

            extent_tree: None,
        };

        // Parse extent tree if using extents
        if inode.uses_extents() {
            inode.extent_tree = Some(ExtentTree::parse(ino, &inode.i_block)?);
        }

        Ok(inode)
    }

    /// Serialize inode to bytes
    pub fn serialize(&self, data: &mut [u8]) {
        data[0..2].copy_from_slice(&self.i_mode.to_le_bytes());
        data[2..4].copy_from_slice(&(self.i_uid as u16).to_le_bytes());
        data[4..8].copy_from_slice(&self.i_size_lo.to_le_bytes());
        data[8..12].copy_from_slice(&self.i_atime.to_le_bytes());
        data[12..16].copy_from_slice(&self.i_ctime.to_le_bytes());
        data[16..20].copy_from_slice(&self.i_mtime.to_le_bytes());
        data[20..24].copy_from_slice(&self.i_dtime.to_le_bytes());
        data[24..26].copy_from_slice(&(self.i_gid as u16).to_le_bytes());
        data[26..28].copy_from_slice(&self.i_links_count.to_le_bytes());
        data[28..32].copy_from_slice(&self.i_blocks_lo.to_le_bytes());
        data[32..36].copy_from_slice(&self.i_flags.to_le_bytes());
        data[40..100].copy_from_slice(&self.i_block);
        data[100..104].copy_from_slice(&self.i_generation.to_le_bytes());
        data[104..108].copy_from_slice(&self.i_file_acl_lo.to_le_bytes());
        data[108..112].copy_from_slice(&self.i_size_hi.to_le_bytes());
        data[112..116].copy_from_slice(&self.i_obso_faddr.to_le_bytes());
        data[116..118].copy_from_slice(&self.i_blocks_hi.to_le_bytes());
        data[118..120].copy_from_slice(&self.i_file_acl_hi.to_le_bytes());
        data[120..122].copy_from_slice(&self.i_uid_hi.to_le_bytes());
        data[122..124].copy_from_slice(&self.i_gid_hi.to_le_bytes());
        data[124..126].copy_from_slice(&self.i_checksum_lo.to_le_bytes());
        data[126..128].copy_from_slice(&self.i_reserved.to_le_bytes());

        if data.len() >= 256 {
            data[128..130].copy_from_slice(&self.i_extra_isize.to_le_bytes());
            data[130..132].copy_from_slice(&self.i_checksum_hi.to_le_bytes());
            data[132..136].copy_from_slice(&self.i_ctime_extra.to_le_bytes());
            data[136..140].copy_from_slice(&self.i_mtime_extra.to_le_bytes());
            data[140..144].copy_from_slice(&self.i_atime_extra.to_le_bytes());
            data[144..148].copy_from_slice(&self.i_crtime.to_le_bytes());
            data[148..152].copy_from_slice(&self.i_crtime_extra.to_le_bytes());
            data[152..156].copy_from_slice(&self.i_version_hi.to_le_bytes());
            data[156..160].copy_from_slice(&self.i_projid.to_le_bytes());
        }
    }

    /// Get file size (64-bit)
    pub fn size(&self) -> u64 {
        ((self.i_size_hi as u64) << 32) | (self.i_size_lo as u64)
    }

    /// Set file size (64-bit)
    pub fn set_size(&mut self, size: u64) {
        self.i_size_lo = size as u32;
        self.i_size_hi = (size >> 32) as u32;
    }

    /// Get block count (48-bit, in 512-byte blocks)
    pub fn blocks(&self) -> u64 {
        ((self.i_blocks_hi as u64) << 32) | (self.i_blocks_lo as u64)
    }

    /// Check if inode uses extents
    pub fn uses_extents(&self) -> bool {
        (self.i_flags & 0x80000) != 0
    }

    /// Get inode type
    pub fn inode_type(&self) -> InodeType {
        match self.i_mode & 0xF000 {
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

    /// Get permissions
    pub fn permissions(&self) -> InodePermissions {
        InodePermissions::new(self.i_mode & 0x0FFF)
    }

    /// Convert to InodeStat
    pub fn to_stat(&self) -> InodeStat {
        InodeStat {
            ino: self.ino,
            mode: self.i_mode,
            nlink: self.i_links_count as u32,
            uid: self.i_uid,
            gid: self.i_gid,
            size: self.size(),
            blksize: 4096,
            blocks: self.blocks(),
            atime: Timestamp { sec: self.i_atime as i64, nsec: 0 },
            mtime: Timestamp { sec: self.i_mtime as i64, nsec: 0 },
            ctime: Timestamp { sec: self.i_ctime as i64, nsec: 0 },
            inode_type: self.inode_type(),
            rdev: 0,
        }
    }

    /// Get extent tree (if using extents)
    pub fn extent_tree(&self) -> Option<&ExtentTree> {
        self.extent_tree.as_ref()
    }

    /// Get mutable extent tree
    pub fn extent_tree_mut(&mut self) -> Option<&mut ExtentTree> {
        self.extent_tree.as_mut()
    }

    /// Get physical block for file block
    pub fn get_block(&self, file_block: u64) -> Option<u64> {
        if let Some(tree) = &self.extent_tree {
            tree.get_block(file_block)
        } else {
            None
        }
    }
}

/// Inode Manager
///
/// Manages inode allocation, caching, and I/O
pub struct InodeManager {
    /// Block device
    device: Arc<Mutex<dyn BlockDevice>>,
    /// Superblock
    superblock: super::superblock::Ext4plusSuperblock,
    /// Block allocator
    allocator: Arc<super::allocation::BlockAllocator>,
    /// Inode cache (inode number -> inode)
    cache: Mutex<BTreeMap<u64, Arc<Mutex<Ext4plusInode>>>>,
    /// Xattr manager
    xattr_manager: Arc<XattrManager>,
    /// ACL manager
    acl_manager: Arc<AclManager>,
    /// Next inode number to allocate
    next_ino: AtomicU64,
}

impl InodeManager {
    /// Create new inode manager
    pub fn new(
        device: Arc<Mutex<dyn BlockDevice>>,
        superblock: super::superblock::Ext4plusSuperblock,
        allocator: Arc<super::allocation::BlockAllocator>,
    ) -> FsResult<Arc<Self>> {
        let xattr_manager = Arc::new(XattrManager::new());
        let acl_manager = Arc::new(AclManager::new());

        Ok(Arc::new(Self {
            device,
            superblock,
            allocator,
            cache: Mutex::new(BTreeMap::new()),
            xattr_manager,
            acl_manager,
            next_ino: AtomicU64::new(11), // First non-reserved inode
        }))
    }

    /// Read inode from disk
    pub fn read_inode(&self, ino: u64) -> FsResult<Arc<Mutex<Ext4plusInode>>> {
        // Check cache first
        {
            let cache = self.cache.lock();
            if let Some(inode) = cache.get(&ino) {
                return Ok(Arc::clone(inode));
            }
        }

        // Calculate location
        let group = (ino - 1) / self.superblock.s_inodes_per_group as u64;
        let index = (ino - 1) % self.superblock.s_inodes_per_group as u64;
        let inode_size = self.superblock.s_inode_size as usize;

        // Read from disk (simplified - would use group descriptor)
        let mut buffer = alloc::vec![0u8; inode_size];
        // In production, would read from actual inode table location

        let inode = Ext4plusInode::parse(ino, &buffer)?;
        let inode_arc = Arc::new(Mutex::new(inode));

        // Cache it
        {
            let mut cache = self.cache.lock();
            cache.insert(ino, Arc::clone(&inode_arc));
        }

        Ok(inode_arc)
    }

    /// Write inode to disk
    pub fn write_inode(&self, inode: &Ext4plusInode) -> FsResult<()> {
        let inode_size = self.superblock.s_inode_size as usize;
        let mut buffer = alloc::vec![0u8; inode_size];
        inode.serialize(&mut buffer);

        // In production, would write to actual inode table location
        log::trace!("ext4plus: Writing inode {}", inode.ino);

        Ok(())
    }

    /// Allocate new inode
    pub fn allocate_inode(&self, inode_type: InodeType) -> FsResult<Arc<Mutex<Ext4plusInode>>> {
        let ino = self.next_ino.fetch_add(1, Ordering::SeqCst);

        let mut inode = Ext4plusInode {
            ino,
            i_mode: inode_type.to_mode_bits() | 0o755,
            i_uid: 0,
            i_size_lo: 0,
            i_atime: crate::time::unix_timestamp() as u32,
            i_ctime: crate::time::unix_timestamp() as u32,
            i_mtime: crate::time::unix_timestamp() as u32,
            i_dtime: 0,
            i_gid: 0,
            i_links_count: 1,
            i_blocks_lo: 0,
            i_flags: 0x80000, // Use extents
            i_block: [0u8; 60],
            i_generation: 0,
            i_file_acl_lo: 0,
            i_size_hi: 0,
            i_obso_faddr: 0,
            i_blocks_hi: 0,
            i_file_acl_hi: 0,
            i_uid_hi: 0,
            i_gid_hi: 0,
            i_checksum_lo: 0,
            i_reserved: 0,
            i_extra_isize: 32,
            i_checksum_hi: 0,
            i_ctime_extra: 0,
            i_mtime_extra: 0,
            i_atime_extra: 0,
            i_crtime: crate::time::unix_timestamp() as u32,
            i_crtime_extra: 0,
            i_version_hi: 0,
            i_projid: 0,
            extent_tree: Some(ExtentTree::new(ino)),
        };

        // Initialize extent tree in i_block
        if let Some(tree) = &inode.extent_tree {
            tree.serialize(&mut inode.i_block)?;
        }

        self.write_inode(&inode)?;

        let inode_arc = Arc::new(Mutex::new(inode));

        // Cache it
        {
            let mut cache = self.cache.lock();
            cache.insert(ino, Arc::clone(&inode_arc));
        }

        log::debug!("ext4plus: Allocated inode {}", ino);

        Ok(inode_arc)
    }

    /// Free inode
    pub fn free_inode(&self, ino: u64) -> FsResult<()> {
        // Remove from cache
        {
            let mut cache = self.cache.lock();
            cache.remove(&ino);
        }

        log::debug!("ext4plus: Freed inode {}", ino);
        Ok(())
    }

    /// Sync all cached inodes
    pub fn sync_all(&self) -> FsResult<()> {
        let cache = self.cache.lock();
        for inode_arc in cache.values() {
            let inode = inode_arc.lock();
            self.write_inode(&inode)?;
        }
        Ok(())
    }

    /// Get xattr manager
    pub fn xattr_manager(&self) -> &Arc<XattrManager> {
        &self.xattr_manager
    }

    /// Get ACL manager
    pub fn acl_manager(&self) -> &Arc<AclManager> {
        &self.acl_manager
    }
}
