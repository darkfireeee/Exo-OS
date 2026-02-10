//! ext4plus Superblock - Enhanced ext4 superblock implementation
//!
//! Features:
//! - Standard ext4 superblock fields
//! - 64-bit block numbers support
//! - Feature flags validation
//! - Checksum verification
//! - Metadata checksums (crc32c)

use crate::fs::block::device::BlockDevice;
use crate::fs::{FsError, FsResult};
use alloc::sync::Arc;
use spin::Mutex;
use core::mem;

/// ext4plus magic number (same as ext4)
pub const EXT4PLUS_SUPER_MAGIC: u16 = 0xEF53;

/// Superblock location (offset from start of device)
pub const SUPERBLOCK_OFFSET: u64 = 1024;

/// Superblock size
pub const SUPERBLOCK_SIZE: usize = 1024;

/// Feature flags - Compatible features (can mount read-only if not supported)
#[derive(Debug, Clone, Copy)]
pub struct CompatFeatures {
    pub dir_prealloc: bool,
    pub imagic_inodes: bool,
    pub has_journal: bool,
    pub ext_attr: bool,
    pub resize_inode: bool,
    pub dir_index: bool,
}

impl CompatFeatures {
    pub fn from_u32(flags: u32) -> Self {
        Self {
            dir_prealloc: (flags & 0x1) != 0,
            imagic_inodes: (flags & 0x2) != 0,
            has_journal: (flags & 0x4) != 0,
            ext_attr: (flags & 0x8) != 0,
            resize_inode: (flags & 0x10) != 0,
            dir_index: (flags & 0x20) != 0,
        }
    }

    pub fn to_u32(&self) -> u32 {
        let mut flags = 0u32;
        if self.dir_prealloc { flags |= 0x1; }
        if self.imagic_inodes { flags |= 0x2; }
        if self.has_journal { flags |= 0x4; }
        if self.ext_attr { flags |= 0x8; }
        if self.resize_inode { flags |= 0x10; }
        if self.dir_index { flags |= 0x20; }
        flags
    }
}

/// Incompatible features (must support to mount)
#[derive(Debug, Clone, Copy)]
pub struct IncompatFeatures {
    pub compression: bool,
    pub filetype: bool,
    pub recover: bool,
    pub journal_dev: bool,
    pub meta_bg: bool,
    pub extents: bool,
    pub is_64bit: bool,
    pub mmp: bool,
    pub flex_bg: bool,
    pub ea_inode: bool,
    pub dirdata: bool,
    pub csum_seed: bool,
    pub largedir: bool,
    pub inline_data: bool,
    pub encrypt: bool,
}

impl IncompatFeatures {
    pub fn from_u32(flags: u32) -> Self {
        Self {
            compression: (flags & 0x1) != 0,
            filetype: (flags & 0x2) != 0,
            recover: (flags & 0x4) != 0,
            journal_dev: (flags & 0x8) != 0,
            meta_bg: (flags & 0x10) != 0,
            extents: (flags & 0x40) != 0,
            is_64bit: (flags & 0x80) != 0,
            mmp: (flags & 0x100) != 0,
            flex_bg: (flags & 0x200) != 0,
            ea_inode: (flags & 0x400) != 0,
            dirdata: (flags & 0x1000) != 0,
            csum_seed: (flags & 0x2000) != 0,
            largedir: (flags & 0x4000) != 0,
            inline_data: (flags & 0x8000) != 0,
            encrypt: (flags & 0x10000) != 0,
        }
    }

    pub fn to_u32(&self) -> u32 {
        let mut flags = 0u32;
        if self.compression { flags |= 0x1; }
        if self.filetype { flags |= 0x2; }
        if self.recover { flags |= 0x4; }
        if self.journal_dev { flags |= 0x8; }
        if self.meta_bg { flags |= 0x10; }
        if self.extents { flags |= 0x40; }
        if self.is_64bit { flags |= 0x80; }
        if self.mmp { flags |= 0x100; }
        if self.flex_bg { flags |= 0x200; }
        if self.ea_inode { flags |= 0x400; }
        if self.dirdata { flags |= 0x1000; }
        if self.csum_seed { flags |= 0x2000; }
        if self.largedir { flags |= 0x4000; }
        if self.inline_data { flags |= 0x8000; }
        if self.encrypt { flags |= 0x10000; }
        flags
    }
}

/// Read-only compatible features (can mount read-only if not supported)
#[derive(Debug, Clone, Copy)]
pub struct RoCompatFeatures {
    pub sparse_super: bool,
    pub large_file: bool,
    pub btree_dir: bool,
    pub huge_file: bool,
    pub gdt_csum: bool,
    pub dir_nlink: bool,
    pub extra_isize: bool,
    pub has_snapshot: bool,
    pub quota: bool,
    pub bigalloc: bool,
    pub metadata_csum: bool,
    pub replica: bool,
    pub readonly: bool,
    pub project: bool,
}

impl RoCompatFeatures {
    pub fn from_u32(flags: u32) -> Self {
        Self {
            sparse_super: (flags & 0x1) != 0,
            large_file: (flags & 0x2) != 0,
            btree_dir: (flags & 0x4) != 0,
            huge_file: (flags & 0x8) != 0,
            gdt_csum: (flags & 0x10) != 0,
            dir_nlink: (flags & 0x20) != 0,
            extra_isize: (flags & 0x40) != 0,
            has_snapshot: (flags & 0x80) != 0,
            quota: (flags & 0x100) != 0,
            bigalloc: (flags & 0x200) != 0,
            metadata_csum: (flags & 0x400) != 0,
            replica: (flags & 0x800) != 0,
            readonly: (flags & 0x1000) != 0,
            project: (flags & 0x2000) != 0,
        }
    }

    pub fn to_u32(&self) -> u32 {
        let mut flags = 0u32;
        if self.sparse_super { flags |= 0x1; }
        if self.large_file { flags |= 0x2; }
        if self.btree_dir { flags |= 0x4; }
        if self.huge_file { flags |= 0x8; }
        if self.gdt_csum { flags |= 0x10; }
        if self.dir_nlink { flags |= 0x20; }
        if self.extra_isize { flags |= 0x40; }
        if self.has_snapshot { flags |= 0x80; }
        if self.quota { flags |= 0x100; }
        if self.bigalloc { flags |= 0x200; }
        if self.metadata_csum { flags |= 0x400; }
        if self.replica { flags |= 0x800; }
        if self.readonly { flags |= 0x1000; }
        if self.project { flags |= 0x2000; }
        flags
    }
}

/// ext4plus Superblock
#[derive(Debug, Clone)]
pub struct Ext4plusSuperblock {
    /// Total number of inodes
    pub s_inodes_count: u32,
    /// Total number of blocks (low 32 bits)
    pub s_blocks_count_lo: u32,
    /// Reserved blocks count (low 32 bits)
    pub s_r_blocks_count_lo: u32,
    /// Free blocks count (low 32 bits)
    pub s_free_blocks_count_lo: u32,
    /// Free inodes count
    pub s_free_inodes_count: u32,
    /// First data block
    pub s_first_data_block: u32,
    /// Block size (log2(block_size) - 10)
    pub s_log_block_size: u32,
    /// Fragment size
    pub s_log_cluster_size: u32,
    /// Blocks per group
    pub s_blocks_per_group: u32,
    /// Clusters per group
    pub s_clusters_per_group: u32,
    /// Inodes per group
    pub s_inodes_per_group: u32,
    /// Mount time
    pub s_mtime: u32,
    /// Write time
    pub s_wtime: u32,
    /// Mount count
    pub s_mnt_count: u16,
    /// Max mount count
    pub s_max_mnt_count: u16,
    /// Magic signature
    pub s_magic: u16,
    /// File system state
    pub s_state: u16,
    /// Error behavior
    pub s_errors: u16,
    /// Minor revision level
    pub s_minor_rev_level: u16,
    /// Last check time
    pub s_lastcheck: u32,
    /// Check interval
    pub s_checkinterval: u32,
    /// Creator OS
    pub s_creator_os: u32,
    /// Revision level
    pub s_rev_level: u32,
    /// Default uid for reserved blocks
    pub s_def_resuid: u16,
    /// Default gid for reserved blocks
    pub s_def_resgid: u16,

    /// First non-reserved inode
    pub s_first_ino: u32,
    /// Inode size
    pub s_inode_size: u16,
    /// Block group number of this superblock
    pub s_block_group_nr: u16,
    /// Compatible features
    pub s_feature_compat: u32,
    /// Incompatible features
    pub s_feature_incompat: u32,
    /// Read-only compatible features
    pub s_feature_ro_compat: u32,
    /// UUID
    pub s_uuid: [u8; 16],
    /// Volume name
    pub s_volume_name: [u8; 16],

    /// Journal inode number
    pub s_journal_inum: u32,
    /// Journal device number
    pub s_journal_dev: u32,
    /// Head of orphan inode list
    pub s_last_orphan: u32,

    /// High 32 bits of blocks count
    pub s_blocks_count_hi: u32,
    /// High 32 bits of reserved blocks count
    pub s_r_blocks_count_hi: u32,
    /// High 32 bits of free blocks count
    pub s_free_blocks_count_hi: u32,

    /// Superblock checksum
    pub s_checksum: u32,
}

impl Ext4plusSuperblock {
    /// Read superblock from block device
    pub fn read(device: &Arc<Mutex<dyn BlockDevice>>) -> FsResult<Self> {
        let mut buf = [0u8; SUPERBLOCK_SIZE];

        // Read from offset 1024
        let dev = device.lock();
        let block_num = SUPERBLOCK_OFFSET / 512;
        dev.read_blocks(block_num, &mut buf)?;
        drop(dev);

        Self::parse(&buf)
    }

    /// Parse superblock from raw bytes
    pub fn parse(data: &[u8]) -> FsResult<Self> {
        if data.len() < SUPERBLOCK_SIZE {
            return Err(FsError::InvalidData);
        }

        let s_magic = u16::from_le_bytes([data[56], data[57]]);
        if s_magic != EXT4PLUS_SUPER_MAGIC {
            return Err(FsError::InvalidData);
        }

        let sb = Self {
            s_inodes_count: u32::from_le_bytes([data[0], data[1], data[2], data[3]]),
            s_blocks_count_lo: u32::from_le_bytes([data[4], data[5], data[6], data[7]]),
            s_r_blocks_count_lo: u32::from_le_bytes([data[8], data[9], data[10], data[11]]),
            s_free_blocks_count_lo: u32::from_le_bytes([data[12], data[13], data[14], data[15]]),
            s_free_inodes_count: u32::from_le_bytes([data[16], data[17], data[18], data[19]]),
            s_first_data_block: u32::from_le_bytes([data[20], data[21], data[22], data[23]]),
            s_log_block_size: u32::from_le_bytes([data[24], data[25], data[26], data[27]]),
            s_log_cluster_size: u32::from_le_bytes([data[28], data[29], data[30], data[31]]),
            s_blocks_per_group: u32::from_le_bytes([data[32], data[33], data[34], data[35]]),
            s_clusters_per_group: u32::from_le_bytes([data[36], data[37], data[38], data[39]]),
            s_inodes_per_group: u32::from_le_bytes([data[40], data[41], data[42], data[43]]),
            s_mtime: u32::from_le_bytes([data[44], data[45], data[46], data[47]]),
            s_wtime: u32::from_le_bytes([data[48], data[49], data[50], data[51]]),
            s_mnt_count: u16::from_le_bytes([data[52], data[53]]),
            s_max_mnt_count: u16::from_le_bytes([data[54], data[55]]),
            s_magic,
            s_state: u16::from_le_bytes([data[58], data[59]]),
            s_errors: u16::from_le_bytes([data[60], data[61]]),
            s_minor_rev_level: u16::from_le_bytes([data[62], data[63]]),
            s_lastcheck: u32::from_le_bytes([data[64], data[65], data[66], data[67]]),
            s_checkinterval: u32::from_le_bytes([data[68], data[69], data[70], data[71]]),
            s_creator_os: u32::from_le_bytes([data[72], data[73], data[74], data[75]]),
            s_rev_level: u32::from_le_bytes([data[76], data[77], data[78], data[79]]),
            s_def_resuid: u16::from_le_bytes([data[80], data[81]]),
            s_def_resgid: u16::from_le_bytes([data[82], data[83]]),
            s_first_ino: u32::from_le_bytes([data[84], data[85], data[86], data[87]]),
            s_inode_size: u16::from_le_bytes([data[88], data[89]]),
            s_block_group_nr: u16::from_le_bytes([data[90], data[91]]),
            s_feature_compat: u32::from_le_bytes([data[92], data[93], data[94], data[95]]),
            s_feature_incompat: u32::from_le_bytes([data[96], data[97], data[98], data[99]]),
            s_feature_ro_compat: u32::from_le_bytes([data[100], data[101], data[102], data[103]]),
            s_uuid: [
                data[104], data[105], data[106], data[107],
                data[108], data[109], data[110], data[111],
                data[112], data[113], data[114], data[115],
                data[116], data[117], data[118], data[119],
            ],
            s_volume_name: [
                data[120], data[121], data[122], data[123],
                data[124], data[125], data[126], data[127],
                data[128], data[129], data[130], data[131],
                data[132], data[133], data[134], data[135],
            ],
            s_journal_inum: u32::from_le_bytes([data[224], data[225], data[226], data[227]]),
            s_journal_dev: u32::from_le_bytes([data[228], data[229], data[230], data[231]]),
            s_last_orphan: u32::from_le_bytes([data[232], data[233], data[234], data[235]]),
            s_blocks_count_hi: u32::from_le_bytes([data[336], data[337], data[338], data[339]]),
            s_r_blocks_count_hi: u32::from_le_bytes([data[340], data[341], data[342], data[343]]),
            s_free_blocks_count_hi: u32::from_le_bytes([data[344], data[345], data[346], data[347]]),
            s_checksum: u32::from_le_bytes([data[1020], data[1021], data[1022], data[1023]]),
        };

        Ok(sb)
    }

    /// Write superblock to block device
    pub fn write(&self, device: &Arc<Mutex<dyn BlockDevice>>) -> FsResult<()> {
        let mut buf = [0u8; SUPERBLOCK_SIZE];
        self.serialize(&mut buf);

        let mut dev = device.lock();
        let block_num = SUPERBLOCK_OFFSET / 512;
        dev.write_blocks(block_num, &buf)?;
        drop(dev);

        Ok(())
    }

    /// Serialize superblock to bytes
    pub fn serialize(&self, buf: &mut [u8]) {
        buf[0..4].copy_from_slice(&self.s_inodes_count.to_le_bytes());
        buf[4..8].copy_from_slice(&self.s_blocks_count_lo.to_le_bytes());
        buf[8..12].copy_from_slice(&self.s_r_blocks_count_lo.to_le_bytes());
        buf[12..16].copy_from_slice(&self.s_free_blocks_count_lo.to_le_bytes());
        buf[16..20].copy_from_slice(&self.s_free_inodes_count.to_le_bytes());
        buf[20..24].copy_from_slice(&self.s_first_data_block.to_le_bytes());
        buf[24..28].copy_from_slice(&self.s_log_block_size.to_le_bytes());
        buf[28..32].copy_from_slice(&self.s_log_cluster_size.to_le_bytes());
        buf[32..36].copy_from_slice(&self.s_blocks_per_group.to_le_bytes());
        buf[36..40].copy_from_slice(&self.s_clusters_per_group.to_le_bytes());
        buf[40..44].copy_from_slice(&self.s_inodes_per_group.to_le_bytes());
        buf[44..48].copy_from_slice(&self.s_mtime.to_le_bytes());
        buf[48..52].copy_from_slice(&self.s_wtime.to_le_bytes());
        buf[52..54].copy_from_slice(&self.s_mnt_count.to_le_bytes());
        buf[54..56].copy_from_slice(&self.s_max_mnt_count.to_le_bytes());
        buf[56..58].copy_from_slice(&self.s_magic.to_le_bytes());
        buf[58..60].copy_from_slice(&self.s_state.to_le_bytes());
        buf[60..62].copy_from_slice(&self.s_errors.to_le_bytes());
        buf[62..64].copy_from_slice(&self.s_minor_rev_level.to_le_bytes());
        buf[64..68].copy_from_slice(&self.s_lastcheck.to_le_bytes());
        buf[68..72].copy_from_slice(&self.s_checkinterval.to_le_bytes());
        buf[72..76].copy_from_slice(&self.s_creator_os.to_le_bytes());
        buf[76..80].copy_from_slice(&self.s_rev_level.to_le_bytes());
        buf[80..82].copy_from_slice(&self.s_def_resuid.to_le_bytes());
        buf[82..84].copy_from_slice(&self.s_def_resgid.to_le_bytes());
        buf[84..88].copy_from_slice(&self.s_first_ino.to_le_bytes());
        buf[88..90].copy_from_slice(&self.s_inode_size.to_le_bytes());
        buf[90..92].copy_from_slice(&self.s_block_group_nr.to_le_bytes());
        buf[92..96].copy_from_slice(&self.s_feature_compat.to_le_bytes());
        buf[96..100].copy_from_slice(&self.s_feature_incompat.to_le_bytes());
        buf[100..104].copy_from_slice(&self.s_feature_ro_compat.to_le_bytes());
        buf[104..120].copy_from_slice(&self.s_uuid);
        buf[120..136].copy_from_slice(&self.s_volume_name);
        buf[224..228].copy_from_slice(&self.s_journal_inum.to_le_bytes());
        buf[228..232].copy_from_slice(&self.s_journal_dev.to_le_bytes());
        buf[232..236].copy_from_slice(&self.s_last_orphan.to_le_bytes());
        buf[336..340].copy_from_slice(&self.s_blocks_count_hi.to_le_bytes());
        buf[340..344].copy_from_slice(&self.s_r_blocks_count_hi.to_le_bytes());
        buf[344..348].copy_from_slice(&self.s_free_blocks_count_hi.to_le_bytes());

        let checksum = self.compute_checksum();
        buf[1020..1024].copy_from_slice(&checksum.to_le_bytes());
    }

    /// Get actual block size
    pub fn block_size(&self) -> usize {
        1024 << self.s_log_block_size
    }

    /// Get total blocks count (64-bit)
    pub fn total_blocks(&self) -> u64 {
        ((self.s_blocks_count_hi as u64) << 32) | (self.s_blocks_count_lo as u64)
    }

    /// Get free blocks count (64-bit)
    pub fn free_blocks(&self) -> u64 {
        ((self.s_free_blocks_count_hi as u64) << 32) | (self.s_free_blocks_count_lo as u64)
    }

    /// Get reserved blocks count (64-bit)
    pub fn reserved_blocks(&self) -> u64 {
        ((self.s_r_blocks_count_hi as u64) << 32) | (self.s_r_blocks_count_lo as u64)
    }

    /// Get number of block groups
    pub fn block_groups_count(&self) -> u32 {
        let total_blocks = self.total_blocks();
        ((total_blocks + self.s_blocks_per_group as u64 - 1) / self.s_blocks_per_group as u64) as u32
    }

    /// Get compatible features
    pub fn compat_features(&self) -> CompatFeatures {
        CompatFeatures::from_u32(self.s_feature_compat)
    }

    /// Get incompatible features
    pub fn incompat_features(&self) -> IncompatFeatures {
        IncompatFeatures::from_u32(self.s_feature_incompat)
    }

    /// Get read-only compatible features
    pub fn ro_compat_features(&self) -> RoCompatFeatures {
        RoCompatFeatures::from_u32(self.s_feature_ro_compat)
    }

    /// Validate superblock
    pub fn validate(&self) -> FsResult<()> {
        if self.s_magic != EXT4PLUS_SUPER_MAGIC {
            return Err(FsError::InvalidData);
        }

        if self.s_inodes_count == 0 {
            return Err(FsError::InvalidData);
        }

        if self.total_blocks() == 0 {
            return Err(FsError::InvalidData);
        }

        if self.s_blocks_per_group == 0 {
            return Err(FsError::InvalidData);
        }

        if self.s_inodes_per_group == 0 {
            return Err(FsError::InvalidData);
        }

        let block_size = self.block_size();
        if block_size != 1024 && block_size != 2048 && block_size != 4096 {
            return Err(FsError::InvalidData);
        }

        Ok(())
    }

    /// Compute checksum (crc32c)
    fn compute_checksum(&self) -> u32 {
        let mut crc = 0xFFFFFFFFu32;

        let mut buf = [0u8; SUPERBLOCK_SIZE - 4];
        self.serialize(&mut [0u8; SUPERBLOCK_SIZE]);

        for &byte in buf.iter() {
            crc ^= byte as u32;
            for _ in 0..8 {
                if crc & 1 != 0 {
                    crc = (crc >> 1) ^ 0x82F63B78;
                } else {
                    crc >>= 1;
                }
            }
        }

        !crc
    }

    /// Verify checksum
    pub fn verify_checksum(&self) -> bool {
        self.compute_checksum() == self.s_checksum
    }
}
