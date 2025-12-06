//! ext4 Superblock

use crate::drivers::block::BlockDevice;
use crate::fs::{FsError, FsResult};
use alloc::sync::Arc;
use spin::Mutex;

/// ext4 Superblock
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Ext4Superblock {
    pub inodes_count: u32,
    pub blocks_count_lo: u32,
    pub r_blocks_count_lo: u32,
    pub free_blocks_count_lo: u32,
    pub free_inodes_count: u32,
    pub first_data_block: u32,
    pub log_block_size: u32,
    pub log_cluster_size: u32,
    pub blocks_per_group: u32,
    pub clusters_per_group: u32,
    pub inodes_per_group: u32,
    pub mtime: u32,
    pub wtime: u32,
    pub mnt_count: u16,
    pub max_mnt_count: u16,
    pub magic: u16,
    pub state: u16,
    pub errors: u16,
    pub minor_rev_level: u16,
    pub lastcheck: u32,
    pub checkinterval: u32,
    pub creator_os: u32,
    pub rev_level: u32,
    pub def_resuid: u16,
    pub def_resgid: u16,
    // Extended fields (rev >= 1)
    pub first_ino: u32,
    pub inode_size: u16,
    pub block_group_nr: u16,
    pub feature_compat: u32,
    pub feature_incompat: u32,
    pub feature_ro_compat: u32,
    pub uuid: [u8; 16],
    pub volume_name: [u8; 16],
    pub last_mounted: [u8; 64],
    pub algorithm_usage_bitmap: u32,
    // Performance hints
    pub prealloc_blocks: u8,
    pub prealloc_dir_blocks: u8,
    pub reserved_gdt_blocks: u16,
    // Journaling
    pub journal_uuid: [u8; 16],
    pub journal_inum: u32,
    pub journal_dev: u32,
    pub last_orphan: u32,
    pub hash_seed: [u32; 4],
    pub def_hash_version: u8,
    pub jnl_backup_type: u8,
    pub desc_size: u16,
    pub default_mount_opts: u32,
    pub first_meta_bg: u32,
    pub mkfs_time: u32,
    pub jnl_blocks: [u32; 17],
    // 64-bit support
    pub blocks_count_hi: u32,
    pub r_blocks_count_hi: u32,
    pub free_blocks_count_hi: u32,
    pub min_extra_isize: u16,
    pub want_extra_isize: u16,
    pub flags: u32,
    pub raid_stride: u16,
    pub mmp_interval: u16,
    pub mmp_block: u64,
    pub raid_stripe_width: u32,
    pub log_groups_per_flex: u8,
    pub checksum_type: u8,
    pub reserved_pad: u16,
    pub kbytes_written: u64,
    pub snapshot_inum: u32,
    pub snapshot_id: u32,
    pub snapshot_r_blocks_count: u64,
    pub snapshot_list: u32,
    pub error_count: u32,
    pub first_error_time: u32,
    pub first_error_ino: u32,
    pub first_error_block: u64,
    pub first_error_func: [u8; 32],
    pub first_error_line: u32,
    pub last_error_time: u32,
    pub last_error_ino: u32,
    pub last_error_line: u32,
    pub last_error_block: u64,
    pub last_error_func: [u8; 32],
    pub mount_opts: [u8; 64],
    pub usr_quota_inum: u32,
    pub grp_quota_inum: u32,
    pub overhead_blocks: u32,
    pub backup_bgs: [u32; 2],
    pub encrypt_algos: [u8; 4],
    pub encrypt_pw_salt: [u8; 16],
    pub lpf_ino: u32,
    pub prj_quota_inum: u32,
    pub checksum_seed: u32,
    pub reserved: [u32; 98],
    pub checksum: u32,
}

impl Ext4Superblock {
    /// Lit le superblock depuis le disque (offset 1024)
    pub fn read(device: &Arc<Mutex<dyn BlockDevice>>) -> FsResult<Self> {
        let mut buffer = alloc::vec![0u8; 1024];
        
        // Read sectors 2-3 (offset 1024, size 1024)
        device.lock().read(2, &mut buffer[0..512])
            .map_err(|_| FsError::IoError)?;
        device.lock().read(3, &mut buffer[512..1024])
            .map_err(|_| FsError::IoError)?;
        
        let sb = unsafe {
            core::ptr::read_unaligned(buffer.as_ptr() as *const Ext4Superblock)
        };
        
        if sb.magic != super::EXT4_SUPER_MAGIC {
            return Err(FsError::InvalidData);
        }
        
        Ok(sb)
    }
    
    /// Écrit le superblock vers le disque
    pub fn write(&self, device: &Arc<Mutex<dyn BlockDevice>>) -> FsResult<()> {
        let mut buffer = alloc::vec![0u8; 1024];
        
        unsafe {
            core::ptr::write_unaligned(buffer.as_mut_ptr() as *mut Ext4Superblock, *self);
        }
        
        device.lock().write(2, &buffer[0..512])
            .map_err(|_| FsError::IoError)?;
        device.lock().write(3, &buffer[512..1024])
            .map_err(|_| FsError::IoError)?;
        
        Ok(())
    }
    
    /// Total blocks (64-bit)
    pub fn blocks_count(&self) -> u64 {
        ((self.blocks_count_hi as u64) << 32) | (self.blocks_count_lo as u64)
    }
    
    /// Free blocks (64-bit)
    pub fn free_blocks_count(&self) -> u64 {
        ((self.free_blocks_count_hi as u64) << 32) | (self.free_blocks_count_lo as u64)
    }
}

/// Group Descriptor
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Ext4GroupDesc {
    pub block_bitmap_lo: u32,
    pub inode_bitmap_lo: u32,
    pub inode_table_lo: u32,
    pub free_blocks_count_lo: u16,
    pub free_inodes_count_lo: u16,
    pub used_dirs_count_lo: u16,
    pub flags: u16,
    pub exclude_bitmap_lo: u32,
    pub block_bitmap_csum_lo: u16,
    pub inode_bitmap_csum_lo: u16,
    pub itable_unused_lo: u16,
    pub checksum: u16,
    // 64-bit fields
    pub block_bitmap_hi: u32,
    pub inode_bitmap_hi: u32,
    pub inode_table_hi: u32,
    pub free_blocks_count_hi: u16,
    pub free_inodes_count_hi: u16,
    pub used_dirs_count_hi: u16,
    pub itable_unused_hi: u16,
    pub exclude_bitmap_hi: u32,
    pub block_bitmap_csum_hi: u16,
    pub inode_bitmap_csum_hi: u16,
    pub reserved: u32,
}
