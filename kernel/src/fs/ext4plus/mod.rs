// kernel/src/fs/ext4plus/mod.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// EXT4+ — module racine (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Arborescence :
//   ext4plus/
//     superblock.rs     — superbloc on-disk, montage, FsType
//     group_desc.rs     — descripteurs de groupes (32-bit + 64-bit)
//     inode/
//       extent.rs       — B-tree extent (lecture + insertion)
//       xattr.rs        — attributs étendus (parse + API)
//       acl.rs          — ACL POSIX.1e
//       ops.rs          — InodeOps + FileOps (read/write via extent)
//     directory/
//       htree.rs        — index HTree (DX) pour répertoires volumineux
//       linear.rs       — scan linéaire + manipulation d'entrées
//       ops.rs          — InodeOps répertoire (lookup/create/mkdir/…)
//     allocation/
//       balloc.rs       — allocateur bloc simple (bitmap de groupe)
//       mballoc.rs      — multi-blocs (buddy system)
//       prealloc.rs     — fenêtres de pré-allocation séquentielles
//
// Point d'entrée : ext4_register_fs() — appeler depuis fs::init()

pub mod superblock;
pub mod group_desc;
pub mod inode;
pub mod directory;
pub mod allocation;

// Re-exports utiles pour le reste du noyau
pub use superblock::{
    Ext4SuperblockDisk, Ext4Superblock, Ext4VfsSuperblock, Ext4FsType,
    ext4_register_fs,
    EXT4_MAGIC, EXT4_SB_OFFSET,
    FEAT_INCOMPAT_EXTENTS, FEAT_INCOMPAT_64BIT, FEAT_INCOMPAT_FLEX_BG,
    FEAT_COMPAT_HAS_JOURNAL, FEAT_COMPAT_DIR_INDEX,
    SB_STATS,
};
pub use group_desc::{
    Ext4GroupDescDisk32, Ext4GroupDescDisk64, Ext4GroupDesc, GroupDescTable, GDT_STATS,
};
pub use inode::{
    Ext4ExtentHeader, Ext4ExtentLeaf, Ext4ExtentIdx, ExtentResult,
    ext4_find_extent, ext4_insert_extent_inline, EXTENT_STATS,
    XattrEntry, XattrNamespace, XattrCache, XATTR_CACHE, XATTR_STATS,
    AclEntry, acl_check, mode_to_acl, acl_to_mode, ACL_STATS,
    Ext4InodeDisk, Ext4InodeOps, INODE_OPS_STATS,
};
pub use directory::{
    dx_hash, htree_find_block, HTREE_STATS,
    DirEntry, parse_dir_block, linear_emit, linear_add_entry, linear_remove_entry, LINEAR_STATS,
    Ext4DirOps, DIR_OPS_STATS,
};
pub use allocation::{
    ext4_alloc_block, ext4_free_block, BALLOC_STATS,
    MBALLOC, MBALLOC_STATS, BUDDY_MAX_ORDER,
    PREALLOC_MGR, PREALLOC_STATS,
};
