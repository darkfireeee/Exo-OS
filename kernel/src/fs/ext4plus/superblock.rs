// kernel/src/fs/ext4plus/superblock.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// EXT4+ SUPERBLOCK — format disque + montage (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Lit et valide le superbloc EXT4/EXT4+ situé à l'offset 1024 de la partition.
// Gère les feature flags (extent tree, 64-bit, journal, dir_htree, etc.).
// Expose ext4_mount() vers le registre VFS.
//
// Offsets du superbloc (struct ext4_super_block, kernel Linux) :
//   0x00  s_inodes_count         u32
//   0x04  s_blocks_count_lo      u32
//   0x14  s_block_size_lo        u32  (log2 - 10)
//   0x1C  s_blocks_per_group     u32
//   0x20  s_inodes_per_group     u32
//   0x38  s_magic                u16  = 0xEF53
//   0x3A  s_state                u16
//   0x58  s_feature_compat       u32
//   0x5C  s_feature_incompat     u32
//   0x60  s_feature_ro_compat    u32
//   0x68  s_volume_name          [u8; 16]
//   0x78  s_journal_inum         u32
//   0x15C s_desc_size            u16  (64-bit mode)
//   0x160 s_blocks_count_hi      u32
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use core::mem::size_of;
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::boxed::Box;
use alloc::string::String;

use crate::fs::core::types::{FsError, FsResult, DevId, InodeNumber, MountFlags, FS_STATS};
use crate::fs::core::vfs::{FsType, Superblock as VfsSuperblock, MOUNT_TABLE, FS_TYPE_REGISTRY};
use crate::fs::block::bio::{Bio, BioOp, BioFlags, BioVec};
use crate::fs::block::queue::submit_bio;
use crate::fs::integrity::checksum::{crc32c, ChecksumKind};
use crate::scheduler::sync::{spinlock::SpinLock, rwlock::RwLock};
use crate::memory::core::types::PhysAddr;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes EXT4
// ─────────────────────────────────────────────────────────────────────────────

pub const EXT4_MAGIC:         u16 = 0xEF53;
pub const EXT4_STATE_CLEAN:   u16 = 0x0001;
pub const EXT4_STATE_ERROR:   u16 = 0x0002;
pub const EXT4_STATE_ORPHAN:  u16 = 0x0004;
pub const EXT4_SB_OFFSET:     u64 = 1024; // octets depuis début partition

// Feature flags compat
pub const FEAT_COMPAT_DIR_PREALLOC:    u32 = 0x0001;
pub const FEAT_COMPAT_IMAGIC_INODES:   u32 = 0x0002;
pub const FEAT_COMPAT_HAS_JOURNAL:     u32 = 0x0004;
pub const FEAT_COMPAT_EXT_ATTR:        u32 = 0x0008;
pub const FEAT_COMPAT_RESIZE_INODE:    u32 = 0x0010;
pub const FEAT_COMPAT_DIR_INDEX:       u32 = 0x0020; // htree

// Feature flags incompat
pub const FEAT_INCOMPAT_COMPRESSION:   u32 = 0x0001;
pub const FEAT_INCOMPAT_FILETYPE:      u32 = 0x0002;
pub const FEAT_INCOMPAT_RECOVER:       u32 = 0x0004;
pub const FEAT_INCOMPAT_JOURNAL_DEV:   u32 = 0x0008;
pub const FEAT_INCOMPAT_META_BG:       u32 = 0x0010;
pub const FEAT_INCOMPAT_EXTENTS:       u32 = 0x0040; // extent tree
pub const FEAT_INCOMPAT_64BIT:         u32 = 0x0080;
pub const FEAT_INCOMPAT_MMP:           u32 = 0x0100;
pub const FEAT_INCOMPAT_FLEX_BG:       u32 = 0x0200;
pub const FEAT_INCOMPAT_INLINE_DATA:   u32 = 0x8000;

// ─────────────────────────────────────────────────────────────────────────────
// Flags INCOMPAT Exo-OS (RÈGLE FS-EXT4P-01 — PRIORITÉ ABSOLUE)
// ─────────────────────────────────────────────────────────────────────────────
//
// Ces flags rendent le format ext4plus ILLISIBLE par Linux standard.
// Un Linux externe qui voit 0x8000 connu (INLINE_DATA) doit aussi vérifier
// s_exo_version != 0 pour distinguer ext4plus d'ext4 classique.
// Les flags 0x10000 et 0x20000 sont ABSOLUMENT inconnus du noyau Linux → refus.

/// Blake3 checksums sur les données (0x8000 = valeur partagée avec INLINE_DATA).
/// La distinction se fait via s_exo_version != 0.
pub const EXT4_FEATURE_INCOMPAT_EXO_BLAKE3:   u32 = 0x0008_0000;
/// Delayed allocation avec writeback thread.
pub const EXT4_FEATURE_INCOMPAT_EXO_DELAYED:  u32 = 0x0010_0000;
/// Reflinks (inodes partageant des blocs physiques — CoW).
pub const EXT4_FEATURE_INCOMPAT_EXO_REFLINK:  u32 = 0x0020_0000;

/// Combinaison obligatoire inscrite dans tout superblock formaté par Exo-OS.
/// Linux voit des flags INCOMPAT inconnus → REFUSES le montage → données sûres.
pub const EXO_REQUIRED_INCOMPAT: u32 =
    EXT4_FEATURE_INCOMPAT_EXO_BLAKE3  |
    EXT4_FEATURE_INCOMPAT_EXO_DELAYED |
    EXT4_FEATURE_INCOMPAT_EXO_REFLINK;

/// Version du format Exo-OS dans le superblock (0 = ext4 standard, 1 = ext4plus v1).
pub const EXO_FORMAT_VERSION: u32 = 1;

// Feature flags ro_compat
pub const FEAT_RO_COMPAT_SPARSE_SUPER: u32 = 0x0001;
pub const FEAT_RO_COMPAT_LARGE_FILE:   u32 = 0x0002;
pub const FEAT_RO_COMPAT_BTREE_DIR:    u32 = 0x0004;
pub const FEAT_RO_COMPAT_HUGE_FILE:    u32 = 0x0008;
pub const FEAT_RO_COMPAT_GDT_CSUM:     u32 = 0x0010;
pub const FEAT_RO_COMPAT_DIR_NLINK:    u32 = 0x0020;
pub const FEAT_RO_COMPAT_EXTRA_ISIZE:  u32 = 0x0040;
pub const FEAT_RO_COMPAT_METADATA_CSUM:u32 = 0x0400;

/// Features incompat supportées par cet impl.
/// Inclut les flags EXO_ propres à ext4plus.
pub const SUPPORTED_INCOMPAT: u32 =
    FEAT_INCOMPAT_FILETYPE              |
    FEAT_INCOMPAT_RECOVER               |
    FEAT_INCOMPAT_EXTENTS               |
    FEAT_INCOMPAT_64BIT                 |
    FEAT_INCOMPAT_FLEX_BG               |
    FEAT_INCOMPAT_INLINE_DATA           |
    EXT4_FEATURE_INCOMPAT_EXO_BLAKE3    |
    EXT4_FEATURE_INCOMPAT_EXO_DELAYED   |
    EXT4_FEATURE_INCOMPAT_EXO_REFLINK;

// ─────────────────────────────────────────────────────────────────────────────
// Ext4Superblock — image on-disk (repr C, 1024 octets)
// ─────────────────────────────────────────────────────────────────────────────

/// Projection en mémoire du superbloc disque (premier KiB).
/// Seuls les champs utiles pour le montage sont explicités ; le reste est
/// représenté comme padding pour maintenir les offsets exacts.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Ext4SuperblockDisk {
    pub s_inodes_count:        u32,  // 0x00
    pub s_blocks_count_lo:     u32,  // 0x04
    pub s_r_blocks_count_lo:   u32,  // 0x08
    pub s_free_blocks_lo:      u32,  // 0x0C
    pub s_free_inodes_count:   u32,  // 0x10
    pub s_first_data_block:    u32,  // 0x14
    pub s_log_block_size:      u32,  // 0x18  (1024 << s_log_block_size)
    pub s_log_cluster_size:    u32,  // 0x1C
    pub s_blocks_per_group:    u32,  // 0x20
    pub s_clusters_per_group:  u32,  // 0x24
    pub s_inodes_per_group:    u32,  // 0x28
    pub s_mtime:               u32,  // 0x2C
    pub s_wtime:               u32,  // 0x30
    pub s_mnt_count:           u16,  // 0x34
    pub s_max_mnt_count:       u16,  // 0x36
    pub s_magic:               u16,  // 0x38
    pub s_state:               u16,  // 0x3A
    pub s_errors:              u16,  // 0x3C
    pub s_minor_rev_level:     u16,  // 0x3E
    pub s_lastcheck:           u32,  // 0x40
    pub s_checkinterval:       u32,  // 0x44
    pub s_creator_os:          u32,  // 0x48
    pub s_rev_level:           u32,  // 0x4C
    pub s_def_resuid:          u16,  // 0x50
    pub s_def_resgid:          u16,  // 0x52
    // EXT4 (rev ≥ 1)
    pub s_first_ino:           u32,  // 0x54
    pub s_inode_size:          u16,  // 0x58
    pub s_block_group_nr:      u16,  // 0x5A
    pub s_feature_compat:      u32,  // 0x5C
    pub s_feature_incompat:    u32,  // 0x60
    pub s_feature_ro_compat:   u32,  // 0x64
    pub s_uuid:                [u8; 16],  // 0x68
    pub s_volume_name:         [u8; 16],  // 0x78
    pub s_last_mounted:        [u8; 64],  // 0x88
    pub s_algo_bitmap:         u32,  // 0xC8
    // Pré-allocation
    pub s_prealloc_blocks:     u8,   // 0xCC
    pub s_prealloc_dir_blocks: u8,   // 0xCD
    pub s_reserved_gdt_blocks: u16,  // 0xCE
    // Journal
    pub s_journal_uuid:        [u8; 16],  // 0xD0
    pub s_journal_inum:        u32,  // 0xE0
    pub s_journal_dev:         u32,  // 0xE4
    pub s_last_orphan:         u32,  // 0xE8
    pub s_hash_seed:           [u32; 4], // 0xEC
    pub s_def_hash_version:    u8,   // 0xFC
    pub s_reserved_char_pad:   u8,   // 0xFD
    pub s_desc_size:           u16,  // 0xFE
    pub s_default_mount_opts:  u32,  // 0x100
    pub s_first_meta_bg:       u32,  // 0x104
    pub s_mkfs_time:           u32,  // 0x108
    pub s_jnl_blocks:          [u32; 17], // 0x10C
    // 64-bit
    pub s_blocks_count_hi:     u32,  // 0x150
    pub s_r_blocks_count_hi:   u32,  // 0x154
    pub s_free_blocks_hi:      u32,  // 0x158
    pub s_min_extra_isize:     u16,  // 0x15C
    pub s_want_extra_isize:    u16,  // 0x15E
    pub s_flags:               u32,  // 0x160
    pub s_raid_stride:         u16,  // 0x164
    pub s_mmp_update_interval: u16,  // 0x166
    pub s_mmp_block:           u64,  // 0x168
    pub s_raid_stripe_width:   u32,  // 0x170
    pub s_log_groups_per_flex: u8,   // 0x174
    pub s_checksum_type:       u8,   // 0x175
    pub s_encryption_level:    u8,   // 0x176
    pub _pad:                  u8,   // 0x177
    pub s_kbytes_written:      u64,  // 0x178
    pub s_snapshot_inum:       u32,  // 0x180
    pub s_snapshot_id:         u32,  // 0x184
    pub s_snapshot_r_blocks:   u64,  // 0x188
    pub s_snapshot_list:       u32,  // 0x190
    pub s_error_count:         u32,  // 0x194
    pub s_first_error_time:    u32,  // 0x198
    pub s_first_error_ino:     u32,  // 0x19C
    pub s_first_error_block:   u64,  // 0x1A0
    pub s_first_error_func:    [u8; 32], // 0x1A8
    pub s_first_error_line:    u32,  // 0x1C8
    pub s_last_error_time:     u32,  // 0x1CC
    pub s_last_error_ino:      u32,  // 0x1D0
    pub s_last_error_line:     u32,  // 0x1D4
    pub s_last_error_block:    u64,  // 0x1D8
    pub s_last_error_func:     [u8; 32], // 0x1E0
    pub s_mount_opts:          [u8; 64], // 0x200
    pub s_usr_quota_inum:      u32,  // 0x240
    pub s_grp_quota_inum:      u32,  // 0x244
    pub s_overhead_clusters:   u32,  // 0x248
    pub s_backup_bgs:          [u32; 2], // 0x24C
    pub s_encrypt_algos:       [u8; 4],  // 0x254
    pub s_encrypt_pw_salt:     [u8; 16], // 0x258
    pub s_lpf_ino:             u32,  // 0x268
    pub s_prj_quota_inum:      u32,  // 0x26C
    pub s_checksum_seed:       u32,  // 0x270
    /// Version du format Exo-OS : 0 = ext4 standard, 1+ = ext4plus propriétaire.
    pub s_exo_version:         u32,        // 0x274
    /// Blake3 du superblock lui-même (validation intégrité au montage).
    pub s_exo_checksum:        [u8; 32],   // 0x278
    pub _reserved:             [u32; 89],  // 0x298 → (98 - 1 - 8) u32, taille totale inchangée
    pub s_checksum:            u32,        // 0x3FC
}

const _: () = assert!(size_of::<Ext4SuperblockDisk>() == 1024);

// ─────────────────────────────────────────────────────────────────────────────
// Ext4FsSuperblock — superbloc en mémoire (analysé + enrichi)
// ─────────────────────────────────────────────────────────────────────────────

pub struct Ext4Superblock {
    pub disk:              Ext4SuperblockDisk,
    pub block_size:        u64,
    pub group_count:       u64,
    pub blocks_count:      u64,
    pub inodes_count:      u32,
    pub inodes_per_group:  u32,
    pub blocks_per_group:  u32,
    pub inode_size:        u16,
    pub first_ino:         u32,
    pub has_extents:       bool,
    pub has_64bit:         bool,
    pub has_journal:       bool,
    pub has_htree:         bool,
    /// Ext4plus : checksums Blake3 sur les blocs (flag EXO_BLAKE3)
    pub has_blake3:        bool,
    /// Ext4plus : delayed allocation (flag EXO_DELAYED)
    pub has_delayed_alloc: bool,
    /// Ext4plus : reflinks CoW (flag EXO_REFLINK)
    pub has_reflinks:      bool,
    pub needs_recovery:    bool,
    pub read_only:         bool,
    pub desc_size:         u16,
    pub dev:               DevId,
    pub dirty:             AtomicBool,
}

impl Ext4Superblock {
    /// Lit le superbloc depuis le périphérique (offset 1 KiB).
    pub fn read_from_dev(dev: DevId, phys: PhysAddr) -> FsResult<Arc<RwLock<Self>>> {
        // Lit 1024 octets depuis le disque
        let bio = Bio {
            id:       0,
            op:       BioOp::Read,
            dev:      dev.0,
            sector:   EXT4_SB_OFFSET / 512,
            vecs:     alloc::vec![BioVec { phys, virt: phys.as_u64(), len: 1024, offset: 0 }],
            flags:    BioFlags::META,
            status:   core::sync::atomic::AtomicU8::new(0),
            bytes:    core::sync::atomic::AtomicU64::new(0),
            callback: None,
            cb_data:  0,
        };
        submit_bio(bio)?;

        // SAFETY: phys pointe sur 1024 octets alloués et remplis par le BIO.
        let disk: Ext4SuperblockDisk = unsafe {
            core::ptr::read(phys.as_u64() as *const Ext4SuperblockDisk)
        };
        Self::parse(dev, disk)
    }

    fn parse(dev: DevId, disk: Ext4SuperblockDisk) -> FsResult<Arc<RwLock<Self>>> {
        if disk.s_magic != EXT4_MAGIC {
            return Err(FsError::InvalidArgument);
        }
        // RÈGLE FS-EXT4P-01 : vérifier que les flags EXO obligatoires sont présents.
        // Un disque formaté par Exo-OS DOIT contenir EXO_REQUIRED_INCOMPAT.
        // Si seulement certains flags sont présents → superblock incohérent.
        let has_exo_version = disk.s_exo_version >= EXO_FORMAT_VERSION;
        let has_exo_flags   = disk.s_feature_incompat & EXO_REQUIRED_INCOMPAT == EXO_REQUIRED_INCOMPAT;
        if has_exo_version != has_exo_flags {
            // Incohérence : version et flags ne correspondent pas → superblock corrompu.
            SB_STATS.errors.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            return Err(FsError::Corrupt);
        }
        let unsupported = disk.s_feature_incompat & !SUPPORTED_INCOMPAT;
        if unsupported != 0 {
            SB_STATS.errors.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            return Err(FsError::NotSupported);
        }
        let block_size = 1024u64 << disk.s_log_block_size;
        let blocks_lo  = disk.s_blocks_count_lo as u64;
        let blocks_hi  = if disk.s_feature_incompat & FEAT_INCOMPAT_64BIT != 0 {
            (disk.s_blocks_count_hi as u64) << 32
        } else { 0 };
        let blocks_count = blocks_lo | blocks_hi;
        let group_count  = (blocks_count + disk.s_blocks_per_group as u64 - 1) / disk.s_blocks_per_group as u64;
        let needs_recovery = disk.s_feature_incompat & FEAT_INCOMPAT_RECOVER != 0;
        let read_only = disk.s_feature_ro_compat & !FEAT_RO_COMPAT_SPARSE_SUPER != 0;
        let desc_size = if disk.s_feature_incompat & FEAT_INCOMPAT_64BIT != 0 {
            disk.s_desc_size.max(64)
        } else { 32 };

        SB_STATS.mounts.fetch_add(1, Ordering::Relaxed);
        Ok(Arc::new(RwLock::new(Self {
            block_size,
            group_count,
            blocks_count,
            inodes_count:     disk.s_inodes_count,
            inodes_per_group: disk.s_inodes_per_group,
            blocks_per_group: disk.s_blocks_per_group,
            inode_size:       disk.s_inode_size,
            first_ino:        disk.s_first_ino,
            has_extents:      disk.s_feature_incompat & FEAT_INCOMPAT_EXTENTS   != 0,
            has_64bit:        disk.s_feature_incompat & FEAT_INCOMPAT_64BIT     != 0,
            has_journal:      disk.s_feature_compat   & FEAT_COMPAT_HAS_JOURNAL != 0,
            has_htree:        disk.s_feature_compat   & FEAT_COMPAT_DIR_INDEX   != 0,
            has_blake3:       disk.s_feature_incompat & EXT4_FEATURE_INCOMPAT_EXO_BLAKE3  != 0,
            has_delayed_alloc:disk.s_feature_incompat & EXT4_FEATURE_INCOMPAT_EXO_DELAYED != 0,
            has_reflinks:     disk.s_feature_incompat & EXT4_FEATURE_INCOMPAT_EXO_REFLINK != 0,
            needs_recovery,
            read_only,
            desc_size,
            dev,
            disk,
            dirty: AtomicBool::new(false),
        })))
    }

    /// Retourne si le FS nécessite un rejouer du journal avant montage en RW.
    pub fn sb_needs_recovery(&self) -> bool { self.needs_recovery }

    /// Écrit le superbloc modifié sur disque (offset 1 KiB).
    pub fn write_back(&self, phys: PhysAddr) -> FsResult<()> {
        // SAFETY: on surécrit le buffer alloué dont phys est l'adresse physique.
        unsafe {
            core::ptr::write(phys.as_u64() as *mut Ext4SuperblockDisk, self.disk);
        }
        let bio = Bio {
            id:       0,
            op:       BioOp::Write,
            dev:      self.dev.0,
            sector:   EXT4_SB_OFFSET / 512,
            vecs:     alloc::vec![BioVec { phys, virt: phys.as_u64(), len: 1024, offset: 0 }],
            flags:    BioFlags::META | BioFlags::FUA,
            status:   core::sync::atomic::AtomicU8::new(0),
            bytes:    core::sync::atomic::AtomicU64::new(0),
            callback: None,
            cb_data:  0,
        };
        submit_bio(bio)?;
        self.dirty.store(false, Ordering::Release);
        SB_STATS.writes.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Ext4FsType — implémentation de FsType pour le registre VFS
// ─────────────────────────────────────────────────────────────────────────────

pub struct Ext4FsType;

impl FsType for Ext4FsType {
    fn name(&self) -> &'static str { "ext4" }
    fn magic(&self) -> u64 { EXT4_MAGIC as u64 }
    fn mount(&self, dev: DevId, _flags: MountFlags, _data: &str) -> FsResult<Arc<dyn VfsSuperblock>> {
        // Alloue un buffer page pour lire le superbloc.
        let phys = crate::memory::core::types::PhysAddr::new(0); // fourni par le MM réel
        let sb   = Ext4Superblock::read_from_dev(dev, phys)?;
        Ok(Arc::new(Ext4VfsSuperblock { sb, dev }))
    }
    fn unmount(&self, _sb: Arc<dyn VfsSuperblock>) -> FsResult<()> {
        SB_STATS.umounts.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

pub struct Ext4VfsSuperblock {
    pub sb:  Arc<RwLock<Ext4Superblock>>,
    pub dev: DevId,
}

impl VfsSuperblock for Ext4VfsSuperblock {
    fn dev(&self)  -> DevId { self.dev }

    fn root_inode(&self) -> FsResult<crate::fs::core::inode::InodeRef> {
        Err(FsError::NotSupported) // implmentation complète dans une prochaine itération
    }

    fn statfs(&self) -> FsResult<crate::fs::core::types::FsStats> {
        let sb = self.sb.read();
        Ok(crate::fs::core::types::FsStats {
            f_type:   EXT4_MAGIC as u64,
            f_bsize:  sb.block_size,
            f_blocks: sb.blocks_count,
            f_bfree:  sb.disk.s_free_blocks_lo as u64,
            f_bavail: sb.disk.s_free_blocks_lo as u64,
            f_files:  sb.inodes_count as u64,
            f_ffree:  sb.disk.s_free_inodes_count as u64,
            f_fsid:   [0; 2],
            f_namelen: 255,
            f_frsize: sb.block_size,
            f_flags:  0,
            f_spare:  [0; 4],
        })
    }

    fn sync_fs(&self, _wait: bool) -> FsResult<()> {
        SB_STATS.sync_calls.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn remount(&self, _flags: MountFlags, _data: &str) -> FsResult<()> {
        Ok(()) // stub
    }

    fn alloc_inode(&self) -> FsResult<crate::fs::core::inode::InodeRef> {
        Err(FsError::NotSupported) // stub
    }

    fn dealloc_inode(&self, _ino: InodeNumber) -> FsResult<()> {
        SB_STATS.umounts.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn write_super(&self) -> FsResult<()> {
        SB_STATS.writes.fetch_add(1, Ordering::Relaxed);
        Ok(()) // stub : appeler write_back() avec une PhysAddr réelle
    }

    fn flags(&self) -> MountFlags {
        if self.sb.read().read_only {
            MountFlags(MountFlags::MS_RDONLY)
        } else {
            MountFlags(0)
        }
    }
}

/// Enregistre le type de FS ext4 dans le registre VFS global.
pub fn ext4_register_fs() {
    FS_TYPE_REGISTRY.register(Arc::new(Ext4FsType));
    SB_STATS.registrations.fetch_add(1, Ordering::Relaxed);
}

// ─────────────────────────────────────────────────────────────────────────────
// SbStats
// ─────────────────────────────────────────────────────────────────────────────

pub struct SbStats {
    pub registrations: AtomicU64,
    pub mounts:        AtomicU64,
    pub umounts:       AtomicU64,
    pub writes:        AtomicU64,
    pub sync_calls:    AtomicU64,
    pub errors:        AtomicU64,
}

impl SbStats {
    pub const fn new() -> Self {
        macro_rules! z { () => { AtomicU64::new(0) }; }
        Self { registrations: z!(), mounts: z!(), umounts: z!(), writes: z!(), sync_calls: z!(), errors: z!() }
    }
}

pub static SB_STATS: SbStats = SbStats::new();
