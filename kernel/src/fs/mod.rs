// kernel/src/fs/mod.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// FS — MODULE RACINE DU SYSTÈME DE FICHIERS  (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Arborescence complète :
//
//   fs/
//   ├── core/          Abstractions VFS : inode, dentry, descriptor, vfs
//   ├── cache/         Page cache, inode cache, dentry cache, buffer, prefetch, éviction
//   ├── io/            Completion, io_uring, zero-copy, AIO, mmap, direct I/O
//   ├── block/         BIO, file d'attente, ordonnanceur (deadline), pilotes de bloc
//   ├── integrity/     Checksums, journal WAL, récupération, scrubbing, healing, validateur
//   ├── pseudo/        procfs, sysfs, devfs, tmpfs
//   ├── ipc_fs/        Shim FS→IPC : pipefs, socketfs (AF_UNIX)
//   ├── ext4plus/      Implémentation EXT4+ (extent, xattr, ACL, HTree, mballoc)
//   └── compatibility/ POSIX 2024 + syscalls Linux étendus
//
// PILOTES FS EXTERNES (ext4 classique, fat32) :
//   → crate « exo-os-drivers » (dossier drivers/ à la racine du projet)
//
// SÉQUENCE D'INITIALISATION (fs_init) :
//   1.  page_cache_init  — alloue les buckets du page cache
//   2.  inode_cache_init — initialise la table de hachage des inodes
//   3.  buffer_cache_init— alloue les buckets du buffer cache
//   4.  devfs_init       — enregistre /dev/null, /dev/zero, /dev/random …
//   5.  tmpfs_init       — monte un tmpfs (256 MiB max)
//   6.  ext4_register_fs — enregistre le type FS « ext4plus » dans FsTypeRegistry
//   7.  vfs_init         — monte le VFS racine (rootfs)
//
// RÈGLES D'ARCHITECTURE :
//   • Couche 3 : dépend uniquement de memory/ + scheduler/ + security/capability/ + process/
//   • ipc_fs/ est le seul sous-module autorisé à importer crate::ipc::*
//   • Jamais de std::sync::* — uniquement SpinLock / RwLock de crate::scheduler::sync
//   • Tous les unsafe{} portent un commentaire // SAFETY:
//   • Chaque module exporte une struct XxxStats + static XXX_STATS
//
// ═══════════════════════════════════════════════════════════════════════════════

// ── Sous-modules ────────────────────────────────────────────────────────────
pub mod core;
pub mod cache;
pub mod io;
pub mod block;
pub mod integrity;
pub mod pseudo;
pub mod ipc_fs;
pub mod ext4plus;
pub mod compatibility;

// ── Re-exports essentiels (utilisés par les syscalls et les serveurs) ────────
pub use core::types::{
    FsError, FsResult, InodeNumber, DevId, FileMode, FileType,
    OpenFlags, SeekWhence, Timespec64, Stat, Dirent64, MountFlags,
    FS_STATS,
};
pub use core::vfs::{
    FsType, Superblock, InodeOps, FileOps, FileHandle,
    MOUNT_TABLE, FS_TYPE_REGISTRY,
    path_lookup, vfs_mount, vfs_umount,
};
pub use core::inode::{Inode, InodeRef, InodeState};
pub use core::dentry::{Dentry, DentryRef, DentryState, DENTRY_CACHE};
pub use core::descriptor::{Fd, FdTable, FdEntry};

pub use cache::PAGE_CACHE;
pub use cache::INODE_HASH_CACHE;
pub use cache::BUFFER_CACHE;
pub use cache::{run_shrinker, ShrinkerTarget};

pub use block::submit_bio;
pub use block::{Bio, BioOp, BioFlags};

pub use integrity::JOURNAL_STATS;
pub use integrity::journal_recovery;
pub use integrity::compute_checksum;

pub use pseudo::procfs::PROC_STATS;
pub use pseudo::sysfs::SYSFS_STATS;
pub use pseudo::devfs::{devfs_init, DEVFS_REGISTRY, DEV_STATS};
pub use pseudo::tmpfs::{tmpfs_init, TMPFS_STATS};

pub use ipc_fs::create_pipe;
pub use ipc_fs::socketpair;

pub use ext4plus::ext4_register_fs;
pub use ext4plus::SB_STATS;

pub use compatibility::posix_open;
pub use compatibility::posix_read;
pub use compatibility::posix_write;
pub use compatibility::POSIX_STATS;
pub use compatibility::linux_statx;
pub use compatibility::LINUX_STATS;

// ── fs_init ──────────────────────────────────────────────────────────────────

/// Initialise le sous-système FS dans l'ordre correct.
/// Appelé depuis `kernel_main()` après l'initialisation du scheduler et de la mémoire.
pub fn fs_init() {
    // 1. Page cache
    cache::page_cache::page_cache_init(65536);  // 65536 pages = 256 MiB à 4 KiB

    // 2. Inode hash cache
    cache::inode_cache::inode_cache_init(8192);

    // 3. Buffer cache (géré par BUFFER_CACHE statique, pas d'init séparée)
    //    — le constructeur lazy s'exécute au premier accès

    // 4. Devfs (/dev/null, /dev/zero, /dev/random, /dev/urandom, /dev/tty)
    devfs_init();

    // 5. Tmpfs (max 256 MiB, utilisé pour /tmp et initrd)
    tmpfs_init(256 * 1024 * 1024);

    // 6. Registre EXT4+
    ext4_register_fs();

    // 7. VFS init (monte rootfs, enregistre les points de montage de base)
    core::vfs::vfs_init();

    FS_STATS.open_files.store(0, ::core::sync::atomic::Ordering::Relaxed);
}
