// drivers/fs/src/ext4/inode.rs — Inode ext4 on-disk  (exo-os-driver-fs)

use core::mem::size_of;

pub const EXT4_ROOT_INO: u32 = 2;
pub const S_IFREG: u16 = 0o100000;
pub const S_IFDIR: u16 = 0o040000;
pub const S_IFLNK: u16 = 0o120000;
pub const EXT4_INODE_EXTENTS_FL: u32 = 0x0008_0000;
pub const EXT4_INODE_INLINE_DATA_FL: u32 = 0x1000_0000;

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Ext4InodeDisk {
    pub i_mode:        u16,
    pub i_uid:         u16,
    pub i_size_lo:     u32,
    pub i_atime:       u32,
    pub i_ctime:       u32,
    pub i_mtime:       u32,
    pub i_dtime:       u32,
    pub i_gid:         u16,
    pub i_links_count: u16,
    pub i_blocks_lo:   u32,
    pub i_flags:       u32,
    pub i_osd1:        u32,
    pub i_block:       [u32; 15],
    pub i_generation:  u32,
    pub i_file_acl_lo: u32,
    pub i_size_hi:     u32,
    pub i_obso_faddr:  u32,
    pub i_osd2:        [u8; 12],
    pub i_extra_isize: u16,
    pub i_checksum_hi: u16,
    pub i_ctime_extra: u32,
    pub i_mtime_extra: u32,
    pub i_atime_extra: u32,
    pub i_crtime:      u32,
    pub i_crtime_extra: u32,
    pub i_version_hi:  u32,
    pub i_projid:      u32,
    // Rembourrage jusqu'à 256 octets.
    pub _pad:          [u8; 100],
}

const _: () = assert!(size_of::<Ext4InodeDisk>() == 256);

impl Ext4InodeDisk {
    pub fn file_size(&self) -> u64 {
        (self.i_size_lo as u64) | ((self.i_size_hi as u64) << 32)
    }

    pub fn is_dir(&self) -> bool {
        (self.i_mode & 0xF000) == S_IFDIR
    }

    pub fn is_file(&self) -> bool {
        (self.i_mode & 0xF000) == S_IFREG
    }

    pub fn uses_extents(&self) -> bool {
        self.i_flags & EXT4_INODE_EXTENTS_FL != 0
    }
}
