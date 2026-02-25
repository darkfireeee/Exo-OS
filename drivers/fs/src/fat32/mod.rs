// drivers/fs/src/fat32/mod.rs
//
// FAT32 — Point d'entrée  (exo-os-driver-fs)
//
// RÈGLES APPLIQUÉES :
//   FS-FAT32-01 : cluster_count >= 65525 → sinon refus (WrongFsType)
//   FS-FAT32-02 : permissions 0o777 — FAT32 n'a pas d'owner UNIX
//   FS-FAT32-03 : noexec immuable — jamais exécuter depuis FAT32
//   FS-FAT32-04 : LFN reconstruits en ordre inverse (dir_entry.rs)
//   FS-FAT32-05 : FAT1 + FAT2 toujours écrites ensemble (fat_table.rs)

pub mod bpb;
pub mod compat;
pub mod fat_table;
pub mod dir_entry;
pub mod cluster;
pub mod alloc;

pub use bpb::{BiosParameterBlock, ParsedBpb, parse_bpb};
pub use compat::{Fat32MountOptions, fat32_verify};
pub use fat_table::{FAT_FREE, FAT_EOC, FAT_BAD, FAT_MASK, is_eoc, is_free, is_bad,
                    read_entry_from_buf, write_entry_to_buf};
pub use dir_entry::{Fat32DirEntry, DirEntryParsed, parse_dir_cluster, lfn_checksum};
pub use cluster::{cluster_to_sector, cluster_is_valid};
pub use alloc::{find_free_cluster, alloc_reset_hint};

// ── permissions FAT32 ────────────────────────────────────────────────────────

/// RÈGLE FS-FAT32-02 : permissions 0o777 pour tous les fichiers FAT32.
/// FAT32 n'a pas de notion de propriétaire UNIX.
pub const FAT32_DEFAULT_PERMISSIONS: u16 = 0o777;

/// RÈGLE FS-FAT32-03 : montage toujours avec MS_NOEXEC.
/// Jamais exécuter un binaire ELF depuis une partition FAT32.
pub const FAT32_MOUNT_NOEXEC: bool = true;
