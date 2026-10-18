#![allow(unused_imports, dead_code)]
use crate::fs::exofs::posix_bridge::inode_emulation::ObjectIno;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

pub const VFS_ROOT_INO: ObjectIno = 1;
pub const VFS_NAME_MAX: usize = 255;
pub const VFS_PATH_MAX: usize = 4096;
pub const VFS_OPEN_MAX: usize = 1024;
pub const VFS_READDIR_BATCH: usize = 64;
/// Magic propre à la couche namespace/VFS.
/// Il est volontairement distinct du magic superblock ExoFS (`0x4558_4F46`).
pub const VFS_MAGIC: u32 = 0x5654_4654; // "VTFT"
pub const VFS_VERSION: u8 = 1;
pub const VFS_NAMESPACE_MAGIC: u32 = 0x5646_534E; // "VFSN"
pub const VFS_NAMESPACE_VERSION: u16 = 1;
pub const VFS_NAMESPACE_HEADER_SIZE: usize = 32;
pub const VFS_NAMESPACE_DIRENT_HEADER_SIZE: usize = 24;
pub const VFS_NAMESPACE_BLOB_TAG: &[u8] = b"exo:posix-vfs-namespace:v1";

// ─────────────────────────────────────────────────────────────────────────────
// Modes et types de fichiers
// ─────────────────────────────────────────────────────────────────────────────

pub mod file_mode {
    pub const S_IFMT: u32 = 0o170000;
    pub const S_IFREG: u32 = 0o100000;
    pub const S_IFDIR: u32 = 0o040000;
    pub const S_IFLNK: u32 = 0o120000;
    pub const S_IRUSR: u32 = 0o000400;
    pub const S_IWUSR: u32 = 0o000200;
    pub const S_IXUSR: u32 = 0o000100;
    pub const S_IRGRP: u32 = 0o000040;
    pub const S_IWGRP: u32 = 0o000020;
    pub const S_IXGRP: u32 = 0o000010;
    pub const S_IROTH: u32 = 0o000004;
    pub const S_IWOTH: u32 = 0o000002;
    pub const S_IXOTH: u32 = 0o000001;
    pub const DEFAULT_DIR: u32 = S_IFDIR | S_IRUSR | S_IWUSR | S_IXUSR;
    pub const DEFAULT_FILE: u32 = S_IFREG | S_IRUSR | S_IWUSR;
}

/// Flags d'ouverture de fichier.
pub mod open_flags {
    pub const O_RDONLY: u32 = 0x0000;
    pub const O_WRONLY: u32 = 0x0001;
    pub const O_RDWR: u32 = 0x0002;
    pub const O_CREAT: u32 = 0x0040;
    pub const O_EXCL: u32 = 0x0080;
    pub const O_TRUNC: u32 = 0x0200;
    pub const O_APPEND: u32 = 0x0400;
    pub const O_NONBLOCK: u32 = 0x0800;
}

// ─────────────────────────────────────────────────────────────────────────────
// Structures POSIX exposées
// ─────────────────────────────────────────────────────────────────────────────

/// Métadonnées d'un inode (stat-like).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VfsInode {
    pub ino: ObjectIno,
    pub size: u64,
    pub mode: u32,
    pub uid: u32,
    pub link_count: u32,
    pub kind: u8,
    pub _pad: [u8; 3],
}

const _: () = assert!(core::mem::size_of::<VfsInode>() == 32);

impl VfsInode {
    pub fn is_dir(&self) -> bool {
        self.mode & file_mode::S_IFMT == file_mode::S_IFDIR
    }
    pub fn is_regular(&self) -> bool {
        self.mode & file_mode::S_IFMT == file_mode::S_IFREG
    }
    pub fn is_symlink(&self) -> bool {
        self.mode & file_mode::S_IFMT == file_mode::S_IFLNK
    }
    pub fn is_readable_by_owner(&self) -> bool {
        self.mode & file_mode::S_IRUSR != 0
    }
    pub fn is_writable_by_owner(&self) -> bool {
        self.mode & file_mode::S_IWUSR != 0
    }
}

/// Entrée de répertoire.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct VfsDirent {
    pub ino: ObjectIno,
    pub kind: u8,
    pub name_len: u8,
    pub _pad: [u8; 6],
    pub name: [u8; VFS_NAME_MAX],
}

const _: () = assert!(
    core::mem::size_of::<VfsDirent>() == 271,
    "VfsDirent ABI size changed — verifier les appels readdir() userspace"
);

impl VfsDirent {
    pub fn get_name(&self) -> &[u8] {
        let len = self.name_len as usize;
        &self.name[..len.min(VFS_NAME_MAX)]
    }
}

impl core::fmt::Debug for VfsDirent {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let ino = self.ino;
        let kind = self.kind;
        let name_len = self.name_len;
        f.debug_struct("VfsDirent")
            .field("ino", &ino)
            .field("kind", &kind)
            .field("name_len", &name_len)
            .finish()
    }
}

/// Descripteur de fichier ouvert.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct VfsFd {
    pub fd: u64,
    pub ino: ObjectIno,
    pub flags: u32,
    pub offset: u64,
    pub pid: u32,
    pub active: bool,
}

