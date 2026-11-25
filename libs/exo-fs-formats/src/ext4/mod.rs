// drivers/fs/src/ext4/mod.rs
//
// EXT4 CLASSIQUE — Point d'entrée  (exo-os-driver-fs)
//
// RÈGLES APPLIQUÉES :
//   FS-EXT4-01 : Flags INCOMPAT inconnus → refus
//   FS-EXT4-02 : Disque ext4plus → refus + message
//   FS-EXT4-03 : Journal non propre → read-only
//   FS-EXT4-04 : Zéro Blake3, zéro delayed alloc

pub mod compat;
pub mod superblock;
pub mod inode;
pub mod extent;
pub mod dir;
pub mod journal;
pub mod xattr;

pub use compat::{Ext4MountMode, verify_before_mount, COMPAT_STATS};
pub use superblock::{Ext4ParsedSb, parse_superblock, EXT4_MAGIC, SB_STATS};
pub use inode::{Ext4InodeDisk, EXT4_ROOT_INO};
pub use extent::{Ext4Extent, find_extent};
pub use dir::{lookup_in_block, DirLookupResult};
pub use journal::{read_journal_state, JournalState};
pub use xattr::{parse_xattrs, XattrItem};
