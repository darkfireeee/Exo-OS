//! # syscall/fs_bridge/types.rs

#![allow(unused_imports)]
use super::*;
use alloc::vec::Vec;
use core::mem::size_of;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;
use spin::Mutex as SpinMutex;

use crate::fs::exofs::cache::BLOB_CACHE;
use crate::fs::exofs::core::{BlobId, ExofsError, ObjectId};
use crate::fs::exofs::path::path_component::{PathComponent, PathComponentBuf};
use crate::fs::exofs::path::path_index::{
    PathIndex, PathIndexHeader, PATH_INDEX_MAGIC, PATH_INDEX_VERSION,
};
use crate::fs::exofs::path::symlink::{
    invalidate_symlink, is_valid_symlink_target, register_symlink, SYMLINK_MAX_DEPTH,
};
use crate::fs::exofs::syscall::object_fd::{open_flags, OBJECT_TABLE};
use crate::fs::exofs::syscall::object_store;
use crate::ipc::core::types::{EndpointId, IpcError};
use crate::syscall::validation::{copy_from_user, copy_to_user, read_user_typed, write_user_typed};

// SAFETY: accès protégé par le SpinMutex ci-dessous.
unsafe impl Sync for CwdMap {}

pub static mut CWD_MAP: CwdMap = CwdMap {
    slots: [const { CwdEntry::empty() }; CWD_MAP_SIZE],
};

/// Erreurs retournées par le bridge fs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsBridgeError {
    /// fs/ non encore initialisé (FS_READY = false).
    NotReady,
    /// Descripteur de fichier invalide.
    BadFd,
    /// Chemin invalide ou trop long.
    BadPath,
    /// Fichier non trouvé.
    NotFound,
    /// Permission refusée.
    PermDenied,
    /// Pointeur userspace invalide (EFAULT).
    Fault,
    /// Argument invalide.
    Invalid,
    /// Espace disque insuffisant.
    NoSpace,
    /// Memoire kernel insuffisante.
    NoMemory,
    /// Existe déjà.
    Exists,
    /// Pas un répertoire.
    NotDir,
    /// Est un répertoire (pas un fichier ordinaire).
    IsDir,
    /// Répertoire non vide.
    NotEmpty,
    /// Boucle de symlinks.
    Loop,
    /// Erreur I/O non spécifiée.
    Io,
    /// Opération non bloquante sans donnée disponible.
    WouldBlock,
    /// Appel bloquant interrompu par un signal.
    Interrupted,
    /// Écriture sur pipe sans lecteur.
    BrokenPipe,
}

impl FsBridgeError {
    /// Convertit en errno POSIX (valeur négative).
    pub fn to_errno(self) -> i64 {
        match self {
            FsBridgeError::NotReady => -11,   // EAGAIN
            FsBridgeError::BadFd => -9,       // EBADF
            FsBridgeError::BadPath => -22,    // EINVAL
            FsBridgeError::NotFound => -2,    // ENOENT
            FsBridgeError::PermDenied => -13, // EACCES
            FsBridgeError::Fault => -14,      // EFAULT
            FsBridgeError::Invalid => -22,    // EINVAL
            FsBridgeError::NoSpace => -28,    // ENOSPC
            FsBridgeError::NoMemory => -12,   // ENOMEM
            FsBridgeError::Exists => -17,     // EEXIST
            FsBridgeError::NotDir => -20,     // ENOTDIR
            FsBridgeError::IsDir => -21,      // EISDIR
            FsBridgeError::NotEmpty => -39,   // ENOTEMPTY
            FsBridgeError::Loop => -40,       // ELOOP
            FsBridgeError::Io => -5,          // EIO
            FsBridgeError::WouldBlock => -11, // EAGAIN
            FsBridgeError::Interrupted => -4, // EINTR
            FsBridgeError::BrokenPipe => -32, // EPIPE
        }
    }
}

pub const SEEK_SET: u32 = 0;

pub const SEEK_CUR: u32 = 1;

pub const SEEK_END: u32 = 2;

pub const AT_FDCWD: i32 = -100;

pub const F_DUPFD: u32 = 0;

pub const F_GETFD: u32 = 1;

pub const F_SETFD: u32 = 2;

pub const F_GETFL: u32 = 3;

pub const F_SETFL: u32 = 4;

pub const F_GETLK: u32 = 5;

pub const F_SETLK: u32 = 6;

pub const F_SETLKW: u32 = 7;

pub const F_RDLCK: i16 = 0;

pub const F_WRLCK: i16 = 1;

pub const F_UNLCK: i16 = 2;

pub const FD_CLOEXEC: u64 = 1;

pub const O_CLOEXEC: u32 = 0o2000000;

pub const O_NONBLOCK: u32 = 0x0800;

pub const EFD_SEMAPHORE: u32 = 0x0001;

pub const LOCK_SH: u32 = 1;

pub const LOCK_EX: u32 = 2;

pub const LOCK_NB: u32 = 4;

pub const LOCK_UN: u32 = 8;

pub const POLLIN: i16 = 0x0001;

pub const POLLOUT: i16 = 0x0004;

pub const POLLNVAL: i16 = 0x0020;

pub const EPOLL_CLOEXEC: u32 = O_CLOEXEC;

pub const EPOLL_CTL_ADD: i32 = 1;

pub const EPOLL_CTL_DEL: i32 = 2;

pub const EPOLL_CTL_MOD: i32 = 3;

pub const FIONREAD: u64 = 0x541B;

pub const FALLOC_FL_KEEP_SIZE: u32 = 0x01;

pub const SYNC_FILE_RANGE_WAIT_BEFORE: u32 = 0x01;

pub const SYNC_FILE_RANGE_WRITE: u32 = 0x02;

pub const SYNC_FILE_RANGE_WAIT_AFTER: u32 = 0x04;

pub const RLIMIT_NOFILE: u32 = 7;

pub const RLIMIT_NLIMITS: u32 = 16;

pub const STAT_BLOCK_SIZE: i64 = 4096;

pub const EXOFS_STATFS_MAGIC: u64 = 0x4558_4F46;

pub const EXOFS_STATFS_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

pub const STAT_MODE_DIR: u32 = 0o040000 | 0o755;

pub const STAT_MODE_FILE: u32 = 0o100000 | 0o644;

pub const STAT_MODE_SYMLINK: u32 = 0o120000 | 0o777;

pub const TTY_PTS0_HANDLE: u32 = 0xffff_ff01;

pub const TTY_SERVER_ENDPOINT_NAME: &[u8] = b"tty_server";

pub const TTY_MSG_READ_LINE: u32 = 0x131;

pub const TTY_MSG_WRITE: u32 = 0x132;

pub const TTY_MSG_IOCTL: u32 = 0x133;

pub const TTY_LINE_MAX: usize = 184;

// PATCH-TTY-01: timeout reduit de 5 s a 500 ms.
// Un write() utilisateur qui attend 5 s sur un tty_server lent gele le shell.
// 500 ms est suffisant pour absorber une contention IPC normale.
pub const TTY_SEND_TIMEOUT_NS: u64 = 500_000_000;

pub const PATH_INDEX_KIND_DIR: u8 = 0;

pub const PATH_INDEX_KIND_FILE: u8 = 1;

pub const PATH_INDEX_KIND_SYMLINK: u8 = 2;

pub const DT_UNKNOWN: u8 = 0;

pub const DT_DIR: u8 = 4;

pub const DT_REG: u8 = 8;

pub const DT_LNK: u8 = 10;

pub const PSEUDO_PIPE_TAG: u8 = 0xF1;

pub const PSEUDO_EVENTFD_TAG: u8 = 0xE7;

pub const PSEUDO_EPOLL_TAG: u8 = 0xE9;

pub const PSEUDO_INOTIFY_TAG: u8 = 0x1D;

pub const PSEUDO_SOCKET_TAG: u8 = 0x5C;

pub const SOCKET_HEADER_LEN: usize = 32;

pub const S_IFMT: u32 = 0o170000;

pub const S_IFIFO: u32 = 0o010000;

pub const S_IFDIR: u32 = 0o040000;

pub const S_IFREG: u32 = 0o100000;

#[cfg(test)]
pub const STAT_MODE_MASK: u32 = 0o170000;

pub static NEXT_PSEUDO_ID: AtomicU64 = AtomicU64::new(1);

pub static POSIX_UMASK: AtomicU32 = AtomicU32::new(0o022);

pub static TTY_ENDPOINT_CACHE: AtomicU64 = AtomicU64::new(0);

const _: () = assert!(size_of::<TtyRequestWire>() == 216);

const _: () = assert!(size_of::<TtyReplyWire>() == 200);

pub static TTY_STDIN: Mutex<TtyInputBuffer> = Mutex::new(TtyInputBuffer::new());

pub fn ipc_to_fs_error(err: IpcError) -> FsBridgeError {
    match err {
        IpcError::WouldBlock | IpcError::QueueEmpty | IpcError::QueueFull | IpcError::Full => {
            FsBridgeError::WouldBlock
        }
        IpcError::Timeout => FsBridgeError::WouldBlock,
        IpcError::OutOfResources | IpcError::ResourceExhausted | IpcError::ShmPoolFull => {
            FsBridgeError::NoMemory
        }
        IpcError::NotFound
        | IpcError::EndpointNotFound
        | IpcError::ChannelClosed
        | IpcError::Closed
        | IpcError::ConnRefused => {
            TTY_ENDPOINT_CACHE.store(0, Ordering::Release);
            FsBridgeError::NotReady
        }
        IpcError::PermissionDenied => FsBridgeError::PermDenied,
        IpcError::MessageTooLarge => FsBridgeError::Invalid,
        _ => FsBridgeError::Invalid,
    }
}

pub fn errno_to_fs_error(errno: i64) -> FsBridgeError {
    match errno {
        -4 => FsBridgeError::Interrupted,
        -9 => FsBridgeError::BadFd,
        -11 => FsBridgeError::WouldBlock,
        -12 => FsBridgeError::NoMemory,
        -13 => FsBridgeError::PermDenied,
        -14 => FsBridgeError::Fault,
        -22 => FsBridgeError::Invalid,
        -32 => FsBridgeError::BrokenPipe,
        _ => FsBridgeError::Io,
    }
}

#[derive(Clone, Copy)]
pub struct ModeRecord {
    pub blob_id: BlobId,
    pub mode: u32,
}

pub static FILE_MODE_TABLE: Mutex<Vec<ModeRecord>> = Mutex::new(Vec::new());

pub fn upsert_mode(blob_id: BlobId, mode: u32) {
    let mut table = FILE_MODE_TABLE.lock();
    for record in table.iter_mut() {
        if record.blob_id == blob_id {
            record.mode = mode;
            return;
        }
    }
    if table.try_reserve(1).is_ok() {
        table.push(ModeRecord { blob_id, mode });
    }
}

pub fn stored_mode(blob_id: &BlobId) -> Option<u32> {
    FILE_MODE_TABLE
        .lock()
        .iter()
        .find(|record| record.blob_id == *blob_id)
        .map(|record| record.mode)
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct LinuxTimespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct LinuxStat {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_nlink: u64,
    pub st_mode: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    pub __pad0: u32,
    pub st_rdev: u64,
    pub st_size: i64,
    pub st_blksize: i64,
    pub st_blocks: i64,
    pub st_atim: LinuxTimespec,
    pub st_mtim: LinuxTimespec,
    pub st_ctim: LinuxTimespec,
    pub __unused: [i64; 3],
}

const _: () = assert!(size_of::<LinuxStat>() == 144);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct LinuxDirent64 {
    pub d_ino: u64,
    pub d_off: i64,
    pub d_reclen: u16,
    pub d_type: u8,
}

pub const DIRENT64_HEADER_SIZE: usize = size_of::<LinuxDirent64>();

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct LinuxIovec {
    pub iov_base: u64,
    pub iov_len: u64,
}

const _: () = assert!(size_of::<LinuxIovec>() == 16);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct LinuxFlock {
    pub l_type: i16,
    pub l_whence: i16,
    pub l_start: i64,
    pub l_len: i64,
    pub l_pid: i32,
}

const _: () = assert!(size_of::<LinuxFlock>() == 32);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct LinuxStatFs {
    pub f_type: u64,
    pub f_bsize: u64,
    pub f_blocks: u64,
    pub f_bfree: u64,
    pub f_bavail: u64,
    pub f_files: u64,
    pub f_ffree: u64,
    pub f_fsid: [i32; 2],
    pub f_namelen: u64,
    pub f_frsize: u64,
    pub f_flags: u64,
    pub f_spare: [u64; 4],
}

const _: () = assert!(size_of::<LinuxStatFs>() == 120);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct LinuxPollFd {
    pub fd: i32,
    pub events: i16,
    pub revents: i16,
}

const _: () = assert!(size_of::<LinuxPollFd>() == 8);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct LinuxEpollEvent {
    pub events: u32,
    pub data: u64,
}

const _: () = assert!(size_of::<LinuxEpollEvent>() == 16);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct LinuxRlimit {
    pub rlim_cur: u64,
    pub rlim_max: u64,
}

const _: () = assert!(size_of::<LinuxRlimit>() == 16);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct LinuxStatxTimestamp {
    pub tv_sec: i64,
    pub tv_nsec: u32,
    pub __reserved: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct LinuxStatx {
    pub stx_mask: u32,
    pub stx_blksize: u32,
    pub stx_attributes: u64,
    pub stx_nlink: u32,
    pub stx_uid: u32,
    pub stx_gid: u32,
    pub stx_mode: u16,
    pub __spare0: u16,
    pub stx_ino: u64,
    pub stx_size: u64,
    pub stx_blocks: u64,
    pub stx_attributes_mask: u64,
    pub stx_atime: LinuxStatxTimestamp,
    pub stx_btime: LinuxStatxTimestamp,
    pub stx_ctime: LinuxStatxTimestamp,
    pub stx_mtime: LinuxStatxTimestamp,
    pub stx_rdev_major: u32,
    pub stx_rdev_minor: u32,
    pub stx_dev_major: u32,
    pub stx_dev_minor: u32,
    pub stx_mnt_id: u64,
    pub stx_dio_mem_align: u32,
    pub stx_dio_offset_align: u32,
    pub __spare3: [u64; 12],
}

const _: () = assert!(size_of::<LinuxStatx>() == 256);

#[inline]
pub fn exofs_to_bridge_error(err: ExofsError) -> FsBridgeError {
    match err {
        ExofsError::ObjectNotFound | ExofsError::BlobNotFound | ExofsError::NotFound => {
            FsBridgeError::NotFound
        }
        ExofsError::PermissionDenied => FsBridgeError::PermDenied,
        ExofsError::InvalidArgument
        | ExofsError::InvalidPathComponent
        | ExofsError::InvalidState
        | ExofsError::InvalidSize
        | ExofsError::PathTooLong
        | ExofsError::OffsetOverflow
        | ExofsError::Overflow
        | ExofsError::Underflow => FsBridgeError::Invalid,
        ExofsError::NoMemory => FsBridgeError::NoMemory,
        ExofsError::NoSpace => FsBridgeError::NoSpace,
        ExofsError::ObjectAlreadyExists | ExofsError::AlreadyExists => FsBridgeError::Exists,
        ExofsError::NotADirectory => FsBridgeError::NotDir,
        ExofsError::DirectoryNotEmpty => FsBridgeError::NotEmpty,
        ExofsError::TooManySymlinks => FsBridgeError::Loop,
        ExofsError::IoError
        | ExofsError::IoFailed
        | ExofsError::NvmeFlushFailed
        | ExofsError::PartialWrite
        | ExofsError::ShortWrite => FsBridgeError::Io,
        _ => FsBridgeError::Io,
    }
}

#[inline]
pub fn statfs_snapshot() -> LinuxStatFs {
    let used = BLOB_CACHE.used_bytes();
    let total = EXOFS_STATFS_TOTAL_BYTES.max(used);
    let bsize = STAT_BLOCK_SIZE as u64;
    let blocks = total / bsize;
    let used_blocks = used.saturating_add(bsize - 1) / bsize;
    let free = blocks.saturating_sub(used_blocks);

    LinuxStatFs {
        f_type: EXOFS_STATFS_MAGIC,
        f_bsize: bsize,
        f_blocks: blocks,
        f_bfree: free,
        f_bavail: free,
        f_files: BLOB_CACHE.n_entries() as u64,
        f_ffree: u64::MAX / 2,
        f_fsid: [0x4558, 0x4F46],
        f_namelen: 255,
        f_frsize: bsize,
        f_flags: 0,
        f_spare: [0; 4],
    }
}

#[inline]
pub fn dirent_type_from_kind(kind: u8) -> u8 {
    match kind {
        PATH_INDEX_KIND_DIR => DT_DIR,
        PATH_INDEX_KIND_FILE => DT_REG,
        PATH_INDEX_KIND_SYMLINK => DT_LNK,
        _ => DT_UNKNOWN,
    }
}

#[inline]
pub fn inode_from_object_id(object_id: &ObjectId) -> u64 {
    let bytes = object_id.as_bytes();
    u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])
}

pub fn default_stat_mode_for_kind(kind: u8, is_dir: bool) -> u32 {
    match kind {
        PATH_INDEX_KIND_DIR => STAT_MODE_DIR,
        PATH_INDEX_KIND_SYMLINK => STAT_MODE_SYMLINK,
        _ if is_dir => STAT_MODE_DIR,
        _ => STAT_MODE_FILE,
    }
}

pub fn stat_mode_for_blob(blob_id: &BlobId, kind: u8, is_dir: bool) -> u32 {
    stored_mode(blob_id).unwrap_or_else(|| default_stat_mode_for_kind(kind, is_dir))
}

#[inline]
pub fn inode_from_blob_id(blob_id: &BlobId) -> u64 {
    let bytes = blob_id.as_bytes();
    u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])
}

#[inline]
pub fn blocks_for_size(size: u64) -> i64 {
    size.saturating_add(511).saturating_div(512) as i64
}

#[inline]
pub fn linux_stat_for_blob_meta(
    blob_id: BlobId,
    size: u64,
    owner_uid: u32,
    kind: u8,
    is_dir: bool,
) -> LinuxStat {
    LinuxStat {
        st_dev: 0,
        st_ino: inode_from_blob_id(&blob_id),
        st_nlink: 1,
        st_mode: stat_mode_for_blob(&blob_id, kind, is_dir),
        st_uid: owner_uid,
        st_gid: owner_uid,
        __pad0: 0,
        st_rdev: 0,
        st_size: size as i64,
        st_blksize: STAT_BLOCK_SIZE,
        st_blocks: blocks_for_size(size),
        st_atim: LinuxTimespec::default(),
        st_mtim: LinuxTimespec::default(),
        st_ctim: LinuxTimespec::default(),
        __unused: [0; 3],
    }
}

#[inline]
pub fn linux_statx_for_blob_meta(
    blob_id: BlobId,
    size: u64,
    owner_uid: u32,
    kind: u8,
    is_dir: bool,
) -> LinuxStatx {
    let stat = linux_stat_for_blob_meta(blob_id, size, owner_uid, kind, is_dir);
    linux_statx_from_stat(stat)
}

#[inline]
pub fn linux_statx_from_stat(stat: LinuxStat) -> LinuxStatx {
    LinuxStatx {
        stx_mask: 0x0000_1FFF,
        stx_blksize: stat.st_blksize as u32,
        stx_attributes: 0,
        stx_nlink: stat.st_nlink as u32,
        stx_uid: stat.st_uid,
        stx_gid: stat.st_gid,
        stx_mode: stat.st_mode as u16,
        __spare0: 0,
        stx_ino: stat.st_ino,
        stx_size: stat.st_size as u64,
        stx_blocks: stat.st_blocks as u64,
        stx_attributes_mask: 0,
        stx_atime: LinuxStatxTimestamp::default(),
        stx_btime: LinuxStatxTimestamp::default(),
        stx_ctime: LinuxStatxTimestamp::default(),
        stx_mtime: LinuxStatxTimestamp::default(),
        stx_rdev_major: 0,
        stx_rdev_minor: 0,
        stx_dev_major: 0,
        stx_dev_minor: 0,
        stx_mnt_id: 1,
        stx_dio_mem_align: 0,
        stx_dio_offset_align: 0,
        __spare3: [0; 12],
    }
}

#[inline]
pub fn vectored_io<F>(iov_ptr: u64, iovcnt: u32, mut op: F) -> Result<i64, FsBridgeError>
where
    F: FnMut(u64, usize) -> Result<i64, FsBridgeError>,
{
    if iovcnt > 1024 {
        return Err(FsBridgeError::Invalid);
    }
    if iovcnt == 0 {
        return Ok(0);
    }
    if iov_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }

    let mut total = 0i64;
    let mut i = 0u32;
    while i < iovcnt {
        let addr = iov_ptr
            .checked_add((i as u64).saturating_mul(size_of::<LinuxIovec>() as u64))
            .ok_or(FsBridgeError::Fault)?;
        let iov = read_user_typed::<LinuxIovec>(addr).map_err(|_| FsBridgeError::Fault)?;
        if iov.iov_len != 0 {
            if iov.iov_len > usize::MAX as u64 {
                return Err(FsBridgeError::Invalid);
            }
            let n = op(iov.iov_base, iov.iov_len as usize)?;
            total = total.checked_add(n).ok_or(FsBridgeError::Invalid)?;
            if n == 0 || n < iov.iov_len as i64 {
                break;
            }
        }
        i = i.wrapping_add(1);
    }
    Ok(total)
}

/// Close the opaque handles removed from a PCB by execve(O_CLOEXEC).
pub fn close_exec_handles_for_pid(pid: u32, handles: &[u64]) {
    for &handle in handles {
        if is_tty_handle_u64(handle) {
            continue;
        }
        if handle <= u32::MAX as u64 {
            let _ = OBJECT_TABLE.close(handle as u32);
        }
        let _ = crate::fs::exofs::posix_bridge::vfs_close(handle);
    }
    let _ = pid;
}

#[inline]
pub fn fs_link_with_follow(
    old_path: &[u8],
    new_path: &[u8],
    follow_last: bool,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    let old_normalized = resolve_path_with_symlinks(old_path, follow_last, false)?;
    let new_normalized = resolve_path_with_symlinks(new_path, false, true)?;
    if new_normalized == b"/" {
        return Err(FsBridgeError::PermDenied);
    }
    if path_entry(&new_normalized).is_ok() {
        return Err(FsBridgeError::Exists);
    }

    let (src_blob_id, src_kind) = path_entry(&old_normalized)?;
    let src_data = snapshot_blob(&src_blob_id)?;
    if src_kind == PATH_INDEX_KIND_DIR || blob_is_directory(&src_data) {
        return Err(FsBridgeError::PermDenied);
    }

    let (new_parent, new_leaf) = split_parent_and_leaf(&new_normalized)?;
    ensure_directory_exists(&new_parent)?;
    upsert_parent_entry(&new_parent, &new_leaf, src_blob_id, src_kind)?;
    let _ = BLOB_CACHE.mark_dirty(&src_blob_id);
    Ok(0)
}

#[inline]
pub fn vec_with_zeroes(len: usize) -> Result<Vec<u8>, FsBridgeError> {
    let mut out = Vec::new();
    out.try_reserve_exact(len)
        .map_err(|_| FsBridgeError::NoMemory)?;
    out.resize(len, 0);
    Ok(out)
}

#[inline]
pub fn read_user_bytes(ptr: u64, len: usize) -> Result<Vec<u8>, FsBridgeError> {
    if ptr == 0 && len != 0 {
        return Err(FsBridgeError::Fault);
    }
    let mut input = vec_with_zeroes(len)?;
    if len != 0 {
        copy_from_user(input.as_mut_ptr(), ptr as *const u8, len)
            .map_err(|_| FsBridgeError::Fault)?;
    }
    Ok(input)
}

#[inline]
pub fn read_signed_offset(ptr: u64) -> Result<u64, FsBridgeError> {
    let value = read_user_typed::<i64>(ptr).map_err(|_| FsBridgeError::Fault)?;
    if value < 0 {
        return Err(FsBridgeError::Invalid);
    }
    Ok(value as u64)
}

/// Convertit un `Result<i64, FsBridgeError>` en code de retour syscall.
#[inline(always)]
pub fn bridge_result(r: Result<i64, FsBridgeError>) -> i64 {
    match r {
        Ok(n) => n,
        Err(e) => e.to_errno(),
    }
}

