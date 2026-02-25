// drivers/fs/src/ext4/superblock.rs
//
// EXT4 CLASSIQUE — Superblock on-disk  (exo-os-driver-fs)
//
// RÈGLE FS-EXT4-01/02/03 : Vérification via compat::verify_before_mount().

use core::mem::size_of;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::FsDriverError;
use super::compat::{verify_before_mount, Ext4MountMode};

pub const EXT4_MAGIC: u16 = 0xEF53;

/// Superblock ext4 on-disk (1024 octets, offset 1024 depuis début du volume).
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Ext4SuperblockDisk {
    pub s_inodes_count:       u32,
    pub s_blocks_count_lo:    u32,
    pub s_r_blocks_count_lo:  u32,
    pub s_free_blocks_count_lo: u32,
    pub s_free_inodes_count:  u32,
    pub s_first_data_block:   u32,
    pub s_log_block_size:     u32,
    pub s_log_cluster_size:   u32,
    pub s_blocks_per_group:   u32,
    pub s_clusters_per_group: u32,
    pub s_inodes_per_group:   u32,
    pub s_mtime:              u32,
    pub s_wtime:              u32,
    pub s_mnt_count:          u16,
    pub s_max_mnt_count:      u16,
    pub s_magic:              u16,   // offset 56 — doit être EXT4_MAGIC = 0xEF53
    pub s_state:              u16,
    pub s_errors:             u16,
    pub s_minor_rev_level:    u16,
    pub s_lastcheck:          u32,
    pub s_checkinterval:      u32,
    pub s_creator_os:         u32,
    pub s_rev_level:          u32,
    pub s_def_resuid:         u16,
    pub s_def_resgid:         u16,
    // Champs rev1+ (s_rev_level >= 1) :
    pub s_first_ino:          u32,
    pub s_inode_size:         u16,
    pub s_block_group_nr:     u16,
    pub s_feature_compat:     u32,
    pub s_feature_incompat:   u32,   // offset 96
    pub s_feature_ro_compat:  u32,
    pub s_uuid:               [u8; 16],
    pub s_volume_name:        [u8; 16],
    pub s_last_mounted:       [u8; 64],
    pub s_algorithm_usage_bitmap: u32,
    pub s_prealloc_blocks:    u8,
    pub s_prealloc_dir_blocks: u8,
    pub s_reserved_gdt_blocks: u16,
    pub s_journal_uuid:       [u8; 16],
    pub s_journal_inum:       u32,
    pub s_journal_dev:        u32,
    pub s_last_orphan:        u32,
    pub s_hash_seed:          [u32; 4],
    pub s_def_hash_version:   u8,
    pub s_jnl_backup_type:    u8,
    pub s_desc_size:          u16,
    pub s_default_mount_opts: u32,
    pub s_first_meta_bg:      u32,
    pub s_mkfs_time:          u32,
    pub s_jnl_blocks:         [u32; 17],
    pub s_blocks_count_hi:    u32,
    pub s_r_blocks_count_hi:  u32,
    pub s_free_blocks_count_hi: u32,
    pub s_min_extra_isize:    u16,
    pub s_want_extra_isize:   u16,
    pub s_flags:              u32,
    pub s_raid_stride:        u16,
    pub s_mmp_update_interval: u16,
    pub s_mmp_block:          u64,
    pub s_raid_stripe_width:  u32,
    pub s_log_groups_per_flex: u8,
    pub s_checksum_type:      u8,
    pub _reserved1:           [u8; 2],
    pub s_kbytes_written:     u64,
    pub s_snapshot_inum:      u32,
    pub s_snapshot_id:        u32,
    pub s_snapshot_r_blocks_count: u64,
    pub s_snapshot_list:      u32,
    pub s_error_count:        u32,
    pub s_first_error_time:   u32,
    pub s_first_error_ino:    u32,
    pub s_first_error_block:  u64,
    pub s_first_error_func:   [u8; 32],
    pub s_first_error_line:   u32,
    pub s_last_error_time:    u32,
    pub s_last_error_ino:     u32,
    pub s_last_error_line:    u32,
    pub s_last_error_block:   u64,
    pub s_last_error_func:    [u8; 32],
    pub s_mount_opts:         [u8; 64],
    pub s_usr_quota_inum:     u32,
    pub s_grp_quota_inum:     u32,
    pub s_overhead_clusters:  u32,
    pub s_backup_bgs:         [u32; 2],
    pub s_encrypt_algos:      [u8; 4],
    pub s_encrypt_pw_salt:    [u8; 16],
    pub s_lpf_ino:            u32,
    pub s_prj_quota_inum:     u32,
    pub s_checksum_seed:      u32,
    pub s_wtime_hi:           u8,
    pub s_mtime_hi:           u8,
    pub s_mkfs_time_hi:       u8,
    pub s_lastcheck_hi:       u8,
    pub s_first_error_time_hi: u8,
    pub s_last_error_time_hi: u8,
    pub _reserved2:           [u8; 2],
    pub s_encoding:           u16,
    pub s_encoding_flags:     u16,
    pub s_orphan_file_inum:   u32,
    /// Champ Exo-OS : non nul si ce disque est ext4plus (RÈGLE FS-EXT4-02).
    pub s_exo_version:        u32,
    pub _padding:             [u8; 360],
    pub s_checksum:           u32,
}

/// Superblock ext4 analysé en mémoire.
#[derive(Clone, Debug)]
pub struct Ext4ParsedSb {
    pub inodes_count:    u32,
    pub block_size:      u32,
    pub inodes_per_group: u32,
    pub inode_size:      u16,
    pub feature_incompat: u32,
    pub feature_ro_compat: u32,
    pub journal_inum:    u32,
    pub mount_mode:      Ext4MountMode,
    pub needs_recovery:  bool,
}

/// Analyse le contenu brut d'un superblock ext4.
/// `raw` : octets du superblock (au moins 1024 octets depuis offset 1024).
pub fn parse_superblock(raw: &[u8; 1024]) -> Result<Ext4ParsedSb, FsDriverError> {
    // SAFETY: raw a exactement 1024 octets, Ext4SuperblockDisk est repr(C,packed).
    let disk: Ext4SuperblockDisk = unsafe {
        core::ptr::read_unaligned(raw.as_ptr() as *const Ext4SuperblockDisk)
    };

    // Magic check.
    if disk.s_magic != EXT4_MAGIC {
        SB_STATS.bad_magic.fetch_add(1, Ordering::Relaxed);
        return Err(FsDriverError::BadSignature);
    }

    let block_size = 1024u32 << disk.s_log_block_size;
    let needs_recovery = (disk.s_feature_incompat & 0x0004) != 0; // INCOMPAT_RECOVER

    let mount_mode = verify_before_mount(
        disk.s_feature_incompat,
        disk.s_exo_version,
        needs_recovery,
    )?;

    SB_STATS.valid_parses.fetch_add(1, Ordering::Relaxed);
    Ok(Ext4ParsedSb {
        inodes_count:    disk.s_inodes_count,
        block_size,
        inodes_per_group: disk.s_inodes_per_group,
        inode_size:       disk.s_inode_size,
        feature_incompat: disk.s_feature_incompat,
        feature_ro_compat: disk.s_feature_ro_compat,
        journal_inum:    disk.s_journal_inum,
        mount_mode,
        needs_recovery,
    })
}

pub struct SbStats {
    pub bad_magic:    AtomicU64,
    pub valid_parses: AtomicU64,
}

impl SbStats {
    pub const fn new() -> Self {
        macro_rules! z { () => { AtomicU64::new(0) }; }
        Self { bad_magic: z!(), valid_parses: z!() }
    }
}

pub static SB_STATS: SbStats = SbStats::new();
