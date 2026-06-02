// syscall/fs_bridge.rs — Interface de synchronisation syscall ↔ fs/
//
// Ce module définit le contrat entre la couche syscall et le module fs/.
// Les appels sont routés vers ExoFS/VFS via `crate::fs::*`, avec une garde de
// readiness pour éviter les accès avant l'initialisation complète du boot.
//
// ARCHITECTURE :
//   syscall/table.rs            → appelle les fonctions de ce module
//   syscall/fs_bridge.rs        → dispatch vers crate::fs
//   crate::fs (couche 4)        → implémentation VFS réelle
//
// RÈGLE FS-BRIDGE-01 : Ce module ne doit JAMAIS importer fs/ directement.
//   Il utilise uniquement des types primitifs (u64, u32, &[u8], i64).
// RÈGLE FS-BRIDGE-02 : Toutes les fonctions retournent `Result<i64, FsBridgeError>`.
//   La valeur `Ok(n)` est le code de retour POSIX (octets, 0 pour succès...).
//   La valeur `Err(...)` est convertie en errno par le syscall handler.
// RÈGLE FS-BRIDGE-03 : `FS_READY.load()` doit retourner `true` avant tout appel.

use alloc::vec::Vec;
use core::mem::size_of;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

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
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
// État du bridge — atomique pour thread-safety
// ─────────────────────────────────────────────────────────────────────────────

/// Indique si le sous-système fs/ est initialisé et prêt.
/// Mis à `true` par `fs_bridge_init()` lors du boot.
static FS_READY: AtomicBool = AtomicBool::new(false);

/// Initialise le bridge fs.
/// À appeler depuis la séquence de boot après `fs::init()`.
///
/// # Safety
/// Doit être appelé exactement une fois depuis le BSP.
pub unsafe fn fs_bridge_init() {
    FS_READY.store(true, Ordering::Release);
}

/// Retourne `true` si le sous-système fs/ est prêt.
#[inline(always)]
pub fn is_fs_ready() -> bool {
    FS_READY.load(Ordering::Acquire)
}

// ─────────────────────────────────────────────────────────────────────────────
// Erreurs du bridge
// ─────────────────────────────────────────────────────────────────────────────

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

const SEEK_SET: u32 = 0;
const SEEK_CUR: u32 = 1;
const SEEK_END: u32 = 2;
const AT_FDCWD: i32 = -100;
const F_DUPFD: u32 = 0;
const F_GETFD: u32 = 1;
const F_SETFD: u32 = 2;
const F_GETFL: u32 = 3;
const F_SETFL: u32 = 4;
const F_GETLK: u32 = 5;
const F_SETLK: u32 = 6;
const F_SETLKW: u32 = 7;
const F_RDLCK: i16 = 0;
const F_WRLCK: i16 = 1;
const F_UNLCK: i16 = 2;
const FD_CLOEXEC: u64 = 1;
const O_CLOEXEC: u32 = 0o2000000;
const O_NONBLOCK: u32 = 0x0800;
const EFD_SEMAPHORE: u32 = 0x0001;
const LOCK_SH: u32 = 1;
const LOCK_EX: u32 = 2;
const LOCK_NB: u32 = 4;
const LOCK_UN: u32 = 8;
const POLLIN: i16 = 0x0001;
const POLLOUT: i16 = 0x0004;
const POLLNVAL: i16 = 0x0020;
const EPOLL_CLOEXEC: u32 = O_CLOEXEC;
const EPOLL_CTL_ADD: i32 = 1;
const EPOLL_CTL_DEL: i32 = 2;
const EPOLL_CTL_MOD: i32 = 3;
const FIONREAD: u64 = 0x541B;
const FALLOC_FL_KEEP_SIZE: u32 = 0x01;
const SYNC_FILE_RANGE_WAIT_BEFORE: u32 = 0x01;
const SYNC_FILE_RANGE_WRITE: u32 = 0x02;
const SYNC_FILE_RANGE_WAIT_AFTER: u32 = 0x04;
const RLIMIT_NOFILE: u32 = 7;
const RLIMIT_NLIMITS: u32 = 16;
const STAT_BLOCK_SIZE: i64 = 4096;
const EXOFS_STATFS_MAGIC: u64 = 0x4558_4F46;
const EXOFS_STATFS_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
const STAT_MODE_DIR: u32 = 0o040000 | 0o755;
const STAT_MODE_FILE: u32 = 0o100000 | 0o644;
const STAT_MODE_SYMLINK: u32 = 0o120000 | 0o777;
pub const TTY_PTS0_HANDLE: u32 = 0xffff_ff01;
const TTY_SERVER_ENDPOINT_NAME: &[u8] = b"tty_server";
const TTY_MSG_READ_LINE: u32 = 0x131;
const TTY_MSG_WRITE: u32 = 0x132;
const TTY_LINE_MAX: usize = 184;
const TTY_SEND_TIMEOUT_NS: u64 = 5_000_000_000;
const PATH_INDEX_KIND_DIR: u8 = 0;
const PATH_INDEX_KIND_FILE: u8 = 1;
const PATH_INDEX_KIND_SYMLINK: u8 = 2;
const DT_UNKNOWN: u8 = 0;
const DT_DIR: u8 = 4;
const DT_REG: u8 = 8;
const DT_LNK: u8 = 10;
const PSEUDO_PIPE_TAG: u8 = 0xF1;
const PSEUDO_EVENTFD_TAG: u8 = 0xE7;
const PSEUDO_EPOLL_TAG: u8 = 0xE9;
const PSEUDO_INOTIFY_TAG: u8 = 0x1D;
const PSEUDO_SOCKET_TAG: u8 = 0x5C;
const SOCKET_HEADER_LEN: usize = 32;
const S_IFMT: u32 = 0o170000;
const S_IFIFO: u32 = 0o010000;
const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
#[cfg(test)]
const STAT_MODE_MASK: u32 = 0o170000;

static NEXT_PSEUDO_ID: AtomicU64 = AtomicU64::new(1);
static POSIX_UMASK: AtomicU32 = AtomicU32::new(0o022);
static TTY_ENDPOINT_CACHE: AtomicU64 = AtomicU64::new(0);

#[repr(C)]
#[derive(Clone, Copy)]
struct TtyRequestWire {
    sender_pid: u32,
    msg_type: u32,
    reply_endpoint: u64,
    a: u64,
    b: u64,
    data: [u8; TTY_LINE_MAX],
}

impl TtyRequestWire {
    const fn zeroed() -> Self {
        Self {
            sender_pid: 0,
            msg_type: 0,
            reply_endpoint: 0,
            a: 0,
            b: 0,
            data: [0; TTY_LINE_MAX],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct TtyReplyWire {
    status: i64,
    signal: u32,
    len: u32,
    data: [u8; TTY_LINE_MAX],
}

const _: () = assert!(size_of::<TtyRequestWire>() == 216);
const _: () = assert!(size_of::<TtyReplyWire>() == 200);

struct TtyInputBuffer {
    data: [u8; TTY_LINE_MAX],
    head: usize,
    len: usize,
}

impl TtyInputBuffer {
    const fn new() -> Self {
        Self {
            data: [0; TTY_LINE_MAX],
            head: 0,
            len: 0,
        }
    }

    fn push_slice(&mut self, bytes: &[u8]) {
        self.head = 0;
        self.len = bytes.len().min(TTY_LINE_MAX);
        self.data[..self.len].copy_from_slice(&bytes[..self.len]);
    }

    fn pop_into(&mut self, out: &mut [u8]) -> usize {
        let n = out.len().min(self.len);
        if n == 0 {
            return 0;
        }
        out[..n].copy_from_slice(&self.data[self.head..self.head + n]);
        self.head += n;
        self.len -= n;
        if self.len == 0 {
            self.head = 0;
        }
        n
    }
}

static TTY_STDIN: Mutex<TtyInputBuffer> = Mutex::new(TtyInputBuffer::new());

#[derive(Clone, Copy)]
struct ResolvedFd {
    handle: u32,
    flags: u32,
}

#[inline]
fn fd_can_read_flags(flags: u32) -> bool {
    let rw = flags & 0x3;
    rw == open_flags::O_RDONLY || rw == open_flags::O_RDWR
}

#[inline]
fn fd_can_write_flags(flags: u32) -> bool {
    let rw = flags & 0x3;
    rw == open_flags::O_WRONLY || rw == open_flags::O_RDWR
}

#[inline]
fn is_tty_handle(handle: u32) -> bool {
    handle == TTY_PTS0_HANDLE
}

#[inline]
fn is_tty_handle_u64(handle: u64) -> bool {
    handle == TTY_PTS0_HANDLE as u64
}

#[inline]
fn is_tty_path(path: &[u8]) -> bool {
    path == b"/dev/pts/0" || path == b"/dev/tty"
}

fn tty_endpoint() -> Result<EndpointId, FsBridgeError> {
    let cached = TTY_ENDPOINT_CACHE.load(Ordering::Acquire);
    if cached != 0 {
        if let Some(endpoint) = EndpointId::new(cached) {
            return Ok(endpoint);
        }
        TTY_ENDPOINT_CACHE.store(0, Ordering::Release);
    }

    let endpoint = crate::ipc::endpoint::lookup_endpoint(TTY_SERVER_ENDPOINT_NAME)
        .ok_or(FsBridgeError::NotReady)?;
    TTY_ENDPOINT_CACHE.store(endpoint.get(), Ordering::Release);
    Ok(endpoint)
}

fn ipc_to_fs_error(err: IpcError) -> FsBridgeError {
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

fn errno_to_fs_error(errno: i64) -> FsBridgeError {
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

fn tty_call(req: &TtyRequestWire) -> Result<TtyReplyWire, FsBridgeError> {
    let endpoint = tty_endpoint()?;
    let request = unsafe {
        core::slice::from_raw_parts(
            req as *const TtyRequestWire as *const u8,
            size_of::<TtyRequestWire>(),
        )
    };
    let mut reply_buf = [0u8; size_of::<TtyReplyWire>()];
    match crate::ipc::rpc::call_raw(endpoint, request, &mut reply_buf) {
        Ok(n) if n >= size_of::<TtyReplyWire>() => {
            Ok(unsafe { core::ptr::read_unaligned(reply_buf.as_ptr() as *const TtyReplyWire) })
        }
        Ok(_) => Err(FsBridgeError::Invalid),
        Err(err) => Err(ipc_to_fs_error(err)),
    }
}

fn tty_send(req: &TtyRequestWire) -> Result<(), FsBridgeError> {
    let endpoint = tty_endpoint()?;
    let request = unsafe {
        core::slice::from_raw_parts(
            req as *const TtyRequestWire as *const u8,
            size_of::<TtyRequestWire>(),
        )
    };
    let deadline =
        crate::scheduler::timer::clock::monotonic_ns().saturating_add(TTY_SEND_TIMEOUT_NS);

    loop {
        match crate::ipc::channel::raw::try_send_raw_nowait(endpoint, request) {
            Ok(_) => return Ok(()),
            Err(IpcError::WouldBlock) | Err(IpcError::QueueFull) | Err(IpcError::Full) => {
                if crate::scheduler::timer::clock::monotonic_ns() >= deadline {
                    return Err(FsBridgeError::WouldBlock);
                }
                unsafe {
                    let _ = crate::scheduler::core::switch::cooperative_reschedule();
                }
            }
            Err(err) => return Err(ipc_to_fs_error(err)),
        }
    }
}

fn tty_write_bytes(pid: u32, bytes: &[u8]) -> Result<i64, FsBridgeError> {
    let mut written = 0usize;
    while written < bytes.len() {
        let n = (bytes.len() - written).min(TTY_LINE_MAX);
        let mut req = TtyRequestWire::zeroed();
        req.sender_pid = pid;
        req.msg_type = TTY_MSG_WRITE;
        req.a = n as u64;
        req.data[..n].copy_from_slice(&bytes[written..written + n]);
        tty_send(&req)?;
        written += n;
    }
    Ok(bytes.len() as i64)
}

fn tty_refill_stdin(pid: u32) -> Result<(), FsBridgeError> {
    let mut req = TtyRequestWire::zeroed();
    req.sender_pid = pid;
    req.msg_type = TTY_MSG_READ_LINE;
    let reply = tty_call(&req)?;
    if reply.status < 0 {
        return Err(errno_to_fs_error(reply.status));
    }
    let n = (reply.len as usize).min(TTY_LINE_MAX);
    if n == 0 {
        return Err(FsBridgeError::WouldBlock);
    }
    TTY_STDIN.lock().push_slice(&reply.data[..n]);
    Ok(())
}

fn tty_read_bytes(buf_ptr: u64, count: usize, pid: u32) -> Result<i64, FsBridgeError> {
    let mut out = [0u8; TTY_LINE_MAX];
    let mut copied = TTY_STDIN
        .lock()
        .pop_into(&mut out[..count.min(TTY_LINE_MAX)]);
    if copied == 0 {
        tty_refill_stdin(pid)?;
        copied = TTY_STDIN
            .lock()
            .pop_into(&mut out[..count.min(TTY_LINE_MAX)]);
    }
    if copied == 0 {
        return Err(FsBridgeError::WouldBlock);
    }
    copy_to_user(buf_ptr as *mut u8, out.as_ptr(), copied).map_err(|_| FsBridgeError::Fault)?;
    Ok(copied as i64)
}

#[inline]
fn fd_table_flags(open_flags_raw: u32, fd_flags: u32) -> u32 {
    (fd_flags & 0x3)
        | (fd_flags & open_flags::O_APPEND)
        | (open_flags_raw & (O_CLOEXEC | O_NONBLOCK))
}

#[inline]
fn process_has_fd_table(pid: u32) -> bool {
    crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(pid))
        .is_some()
}

#[inline]
fn process_fd_descriptor(pid: u32, fd: u32) -> Option<crate::process::core::pcb::FileDescriptor> {
    crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(pid))
        .and_then(|pcb| pcb.files.lock().get(fd as i32).copied())
}

#[inline]
fn resolve_fd(pid: u32, fd: u32) -> Result<ResolvedFd, FsBridgeError> {
    if let Some(desc) = process_fd_descriptor(pid, fd) {
        if desc.handle > u32::MAX as u64 {
            return Err(FsBridgeError::BadFd);
        }
        return Ok(ResolvedFd {
            handle: desc.handle as u32,
            flags: desc.flags,
        });
    }

    if fd <= 2 {
        return Err(FsBridgeError::BadFd);
    }
    Ok(ResolvedFd {
        handle: fd,
        flags: open_flags::O_RDWR,
    })
}

#[inline]
fn install_process_fd(pid: u32, handle: u64, flags: u32) -> Option<i32> {
    crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(pid))
        .and_then(|pcb| {
            let mut files = pcb.files.lock();
            let fd = files.install(handle, flags);
            (fd >= 0).then_some(fd)
        })
}

#[inline]
fn install_process_fd_at(pid: u32, fd: u32, handle: u64, flags: u32) -> bool {
    crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(pid))
        .map(|pcb| pcb.files.lock().install_at(fd as i32, handle, flags))
        .unwrap_or(false)
}

#[inline]
fn close_process_fd(pid: u32, fd: u32) -> Option<u64> {
    crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(pid))
        .and_then(|pcb| pcb.files.lock().close(fd as i32))
}

#[inline]
fn set_process_fd_flags(pid: u32, fd: u32, flags: u32) -> bool {
    crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(pid))
        .map(|pcb| pcb.files.lock().set_flags(fd as i32, flags))
        .unwrap_or(false)
}

#[derive(Clone, Copy)]
struct ModeRecord {
    blob_id: BlobId,
    mode: u32,
}

static FILE_MODE_TABLE: Mutex<Vec<ModeRecord>> = Mutex::new(Vec::new());

#[inline]
fn process_umask(pid: u32) -> u32 {
    crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(pid))
        .map(|pcb| pcb.umask())
        .unwrap_or_else(|| POSIX_UMASK.load(Ordering::Acquire))
}

#[inline]
fn swap_process_umask(pid: u32, mask: u32) -> u32 {
    match crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(pid))
    {
        Some(pcb) => pcb.swap_umask(mask),
        None => POSIX_UMASK.swap(mask & 0o777, Ordering::AcqRel),
    }
}

#[inline]
fn apply_umask(mode: u32, default_perms: u32, pid: u32) -> u32 {
    let requested = if mode & 0o777 != 0 {
        mode & 0o777
    } else {
        default_perms
    };
    requested & !process_umask(pid) & 0o777
}

fn upsert_mode(blob_id: BlobId, mode: u32) {
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

fn stored_mode(blob_id: &BlobId) -> Option<u32> {
    FILE_MODE_TABLE
        .lock()
        .iter()
        .find(|record| record.blob_id == *blob_id)
        .map(|record| record.mode)
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxTimespec {
    tv_sec: i64,
    tv_nsec: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxStat {
    st_dev: u64,
    st_ino: u64,
    st_nlink: u64,
    st_mode: u32,
    st_uid: u32,
    st_gid: u32,
    __pad0: u32,
    st_rdev: u64,
    st_size: i64,
    st_blksize: i64,
    st_blocks: i64,
    st_atim: LinuxTimespec,
    st_mtim: LinuxTimespec,
    st_ctim: LinuxTimespec,
    __unused: [i64; 3],
}

const _: () = assert!(size_of::<LinuxStat>() == 144);

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxDirent64 {
    d_ino: u64,
    d_off: i64,
    d_reclen: u16,
    d_type: u8,
}

const DIRENT64_HEADER_SIZE: usize = size_of::<LinuxDirent64>();

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxIovec {
    iov_base: u64,
    iov_len: u64,
}

const _: () = assert!(size_of::<LinuxIovec>() == 16);

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxFlock {
    l_type: i16,
    l_whence: i16,
    l_start: i64,
    l_len: i64,
    l_pid: i32,
}

const _: () = assert!(size_of::<LinuxFlock>() == 32);

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxStatFs {
    f_type: u64,
    f_bsize: u64,
    f_blocks: u64,
    f_bfree: u64,
    f_bavail: u64,
    f_files: u64,
    f_ffree: u64,
    f_fsid: [i32; 2],
    f_namelen: u64,
    f_frsize: u64,
    f_flags: u64,
    f_spare: [u64; 4],
}

const _: () = assert!(size_of::<LinuxStatFs>() == 120);

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxPollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

const _: () = assert!(size_of::<LinuxPollFd>() == 8);

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxEpollEvent {
    events: u32,
    data: u64,
}

const _: () = assert!(size_of::<LinuxEpollEvent>() == 16);

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxRlimit {
    rlim_cur: u64,
    rlim_max: u64,
}

const _: () = assert!(size_of::<LinuxRlimit>() == 16);

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxStatxTimestamp {
    tv_sec: i64,
    tv_nsec: u32,
    __reserved: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxStatx {
    stx_mask: u32,
    stx_blksize: u32,
    stx_attributes: u64,
    stx_nlink: u32,
    stx_uid: u32,
    stx_gid: u32,
    stx_mode: u16,
    __spare0: u16,
    stx_ino: u64,
    stx_size: u64,
    stx_blocks: u64,
    stx_attributes_mask: u64,
    stx_atime: LinuxStatxTimestamp,
    stx_btime: LinuxStatxTimestamp,
    stx_ctime: LinuxStatxTimestamp,
    stx_mtime: LinuxStatxTimestamp,
    stx_rdev_major: u32,
    stx_rdev_minor: u32,
    stx_dev_major: u32,
    stx_dev_minor: u32,
    stx_mnt_id: u64,
    stx_dio_mem_align: u32,
    stx_dio_offset_align: u32,
    __spare3: [u64; 12],
}

const _: () = assert!(size_of::<LinuxStatx>() == 256);

#[inline]
fn exofs_to_bridge_error(err: ExofsError) -> FsBridgeError {
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
fn validate_path(path: &[u8]) -> Result<(), FsBridgeError> {
    if path.is_empty() || path.len() > 4096 || path.contains(&0) {
        return Err(FsBridgeError::BadPath);
    }
    Ok(())
}

#[inline]
fn normalize_path_buf(path: &[u8]) -> Result<PathComponentBuf, FsBridgeError> {
    validate_path(path)?;
    let mut buf = PathComponentBuf::from_path(path).map_err(exofs_to_bridge_error)?;
    buf.normalize().map_err(exofs_to_bridge_error)?;
    Ok(buf)
}

#[inline]
fn path_buf_to_bytes(buf: &PathComponentBuf) -> Result<Vec<u8>, FsBridgeError> {
    buf.to_bytes().map_err(exofs_to_bridge_error)
}

#[inline]
fn normalized_path_bytes(path: &[u8]) -> Result<Vec<u8>, FsBridgeError> {
    let buf = normalize_path_buf(path)?;
    path_buf_to_bytes(&buf)
}

#[inline]
fn split_parent_and_leaf(path: &[u8]) -> Result<(Vec<u8>, PathComponent), FsBridgeError> {
    let buf = normalize_path_buf(path)?;
    let leaf = buf.last().cloned().ok_or(FsBridgeError::BadPath)?;
    let mut parent_buf = PathComponentBuf::new();
    for comp in buf.parent() {
        parent_buf
            .push(comp.clone())
            .map_err(exofs_to_bridge_error)?;
    }
    let parent_path = path_buf_to_bytes(&parent_buf)?;
    Ok((parent_path, leaf))
}

#[inline]
fn blob_id_for_path(path: &[u8]) -> Result<BlobId, FsBridgeError> {
    validate_path(path)?;
    Ok(BlobId::from_bytes_blake3(path))
}

#[inline]
fn blob_len(blob_id: &BlobId) -> usize {
    BLOB_CACHE
        .len(blob_id)
        .or_else(|| object_store::persisted_size(blob_id).map(|len| len as usize))
        .unwrap_or(0)
}

#[inline]
fn ensure_blob_exists(blob_id: BlobId) -> Result<(), FsBridgeError> {
    if BLOB_CACHE.contains(&blob_id) {
        return Ok(());
    }
    if let Some(data) =
        object_store::load_blob_data_if_available(&blob_id).map_err(exofs_to_bridge_error)?
    {
        return BLOB_CACHE
            .insert(blob_id, data)
            .map_err(exofs_to_bridge_error);
    }
    BLOB_CACHE
        .insert(blob_id, Vec::new())
        .map_err(exofs_to_bridge_error)
}

#[inline]
fn snapshot_blob(blob_id: &BlobId) -> Result<Vec<u8>, FsBridgeError> {
    if let Some(data) = BLOB_CACHE.get(blob_id) {
        return Ok(data.to_vec());
    }

    if let Some(data) =
        object_store::load_blob_data_if_available(blob_id).map_err(exofs_to_bridge_error)?
    {
        BLOB_CACHE
            .insert(*blob_id, data.clone())
            .map_err(exofs_to_bridge_error)?;
        return Ok(data);
    }

    Err(FsBridgeError::NotFound)
}

#[inline]
fn resize_regular_blob(blob_id: BlobId, length: u64) -> Result<(), FsBridgeError> {
    if length > i64::MAX as u64 || length > usize::MAX as u64 {
        return Err(FsBridgeError::Invalid);
    }

    ensure_blob_exists(blob_id)?;
    if blob_is_directory_by_id(&blob_id) {
        return Err(FsBridgeError::IsDir);
    }

    BLOB_CACHE
        .resize(blob_id, length as usize)
        .map_err(exofs_to_bridge_error)?;
    Ok(())
}

#[inline]
fn read_blob_at(
    blob_id: BlobId,
    offset: u64,
    buf_ptr: u64,
    count: usize,
) -> Result<i64, FsBridgeError> {
    if buf_ptr == 0 && count != 0 {
        return Err(FsBridgeError::Fault);
    }
    if count == 0 {
        return Ok(0);
    }

    if offset > usize::MAX as u64 {
        return Err(FsBridgeError::Invalid);
    }

    let data = BLOB_CACHE
        .read_at(&blob_id, offset as usize, count)
        .map_err(exofs_to_bridge_error)?;
    if data.is_empty() {
        return Ok(0);
    }
    copy_to_user(buf_ptr as *mut u8, data.as_ptr(), data.len())
        .map_err(|_| FsBridgeError::Fault)?;
    Ok(data.len() as i64)
}

#[inline]
fn write_blob_at(
    blob_id: BlobId,
    offset: u64,
    buf_ptr: u64,
    count: usize,
) -> Result<i64, FsBridgeError> {
    if buf_ptr == 0 && count != 0 {
        return Err(FsBridgeError::Fault);
    }
    if count == 0 {
        return Ok(0);
    }
    if offset > usize::MAX as u64 {
        return Err(FsBridgeError::Invalid);
    }

    let input = read_user_bytes(buf_ptr, count)?;

    let start = offset as usize;
    BLOB_CACHE
        .write_at(blob_id, start, &input)
        .map_err(exofs_to_bridge_error)?;
    Ok(count as i64)
}

#[inline]
fn read_blob_bytes_at(
    blob_id: BlobId,
    offset: u64,
    count: usize,
) -> Result<Vec<u8>, FsBridgeError> {
    if count == 0 {
        return Ok(Vec::new());
    }
    if offset > usize::MAX as u64 {
        return Err(FsBridgeError::Invalid);
    }
    BLOB_CACHE
        .read_at(&blob_id, offset as usize, count)
        .map_err(exofs_to_bridge_error)
}

#[inline]
fn write_blob_bytes_at(blob_id: BlobId, offset: u64, bytes: &[u8]) -> Result<i64, FsBridgeError> {
    if bytes.is_empty() {
        return Ok(0);
    }
    if offset > usize::MAX as u64 {
        return Err(FsBridgeError::Invalid);
    }

    let start = offset as usize;
    BLOB_CACHE
        .write_at(blob_id, start, bytes)
        .map_err(exofs_to_bridge_error)?;
    Ok(bytes.len() as i64)
}

#[inline]
fn pseudo_blob_id(tag: u8, seq: u64) -> BlobId {
    let mut bytes = [0u8; 32];
    bytes[0] = tag;
    bytes[1] = b'E';
    bytes[2] = b'X';
    bytes[3] = b'O';
    bytes[4..12].copy_from_slice(&seq.to_le_bytes());
    BlobId(bytes)
}

#[inline]
fn next_pseudo_blob(tag: u8) -> BlobId {
    let seq = NEXT_PSEUDO_ID.fetch_add(1, Ordering::Relaxed);
    pseudo_blob_id(tag, seq)
}

#[inline]
fn is_pseudo_blob(blob_id: &BlobId, tag: u8) -> bool {
    let bytes = blob_id.as_bytes();
    bytes[0] == tag && bytes[1] == b'E' && bytes[2] == b'X' && bytes[3] == b'O'
}

#[inline]
fn eventfd_state(blob_id: BlobId) -> Result<(u64, u32), FsBridgeError> {
    let data = snapshot_blob(&blob_id)?;
    if data.len() < 8 {
        return Err(FsBridgeError::Invalid);
    }
    let value = u64::from_le_bytes([
        data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
    ]);
    let flags = if data.len() > 8 { data[8] as u32 } else { 0 };
    Ok((value, flags))
}

#[inline]
fn store_eventfd_state(blob_id: BlobId, value: u64, flags: u32) -> Result<(), FsBridgeError> {
    let mut data = Vec::new();
    data.extend_from_slice(&value.to_le_bytes());
    data.push((flags & EFD_SEMAPHORE) as u8);
    BLOB_CACHE
        .insert(blob_id, data)
        .map_err(exofs_to_bridge_error)?;
    let _ = BLOB_CACHE.mark_dirty(&blob_id);
    Ok(())
}

#[inline]
fn socket_blob_with_peer(peer: BlobId) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(peer.as_bytes());
    data
}

#[inline]
fn socket_peer_blob(blob_id: BlobId) -> Result<BlobId, FsBridgeError> {
    let data = snapshot_blob(&blob_id)?;
    if data.len() < SOCKET_HEADER_LEN {
        return Err(FsBridgeError::Invalid);
    }
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&data[..SOCKET_HEADER_LEN]);
    Ok(BlobId(bytes))
}

#[inline]
fn socket_payload_len(blob_id: BlobId) -> usize {
    BLOB_CACHE
        .get(&blob_id)
        .map(|data| data.len().saturating_sub(SOCKET_HEADER_LEN))
        .unwrap_or(0)
}

#[inline]
fn read_socket_payload(
    blob_id: BlobId,
    count: usize,
    consume: bool,
) -> Result<Vec<u8>, FsBridgeError> {
    let data = snapshot_blob(&blob_id)?;
    if data.len() <= SOCKET_HEADER_LEN {
        return Ok(Vec::new());
    }
    let read_len = count.min(data.len() - SOCKET_HEADER_LEN);
    let out = data[SOCKET_HEADER_LEN..SOCKET_HEADER_LEN + read_len].to_vec();
    if consume {
        let mut next = data[..SOCKET_HEADER_LEN].to_vec();
        next.extend_from_slice(&data[SOCKET_HEADER_LEN + read_len..]);
        BLOB_CACHE
            .insert(blob_id, next)
            .map_err(exofs_to_bridge_error)?;
        let _ = BLOB_CACHE.mark_dirty(&blob_id);
    }
    Ok(out)
}

#[inline]
fn append_socket_payload(blob_id: BlobId, bytes: &[u8]) -> Result<i64, FsBridgeError> {
    if bytes.is_empty() {
        return Ok(0);
    }
    let mut data = snapshot_blob(&blob_id)?;
    if data.len() < SOCKET_HEADER_LEN {
        return Err(FsBridgeError::Invalid);
    }
    data.try_reserve(bytes.len())
        .map_err(|_| FsBridgeError::NoSpace)?;
    data.extend_from_slice(bytes);
    BLOB_CACHE
        .insert(blob_id, data)
        .map_err(exofs_to_bridge_error)?;
    let _ = BLOB_CACHE.mark_dirty(&blob_id);
    Ok(bytes.len() as i64)
}

#[inline]
fn read_pipe_payload(
    blob_id: BlobId,
    count: usize,
    consume: bool,
) -> Result<Vec<u8>, FsBridgeError> {
    let data = BLOB_CACHE
        .get(&blob_id)
        .map(|bytes| bytes.to_vec())
        .unwrap_or_default();
    if data.is_empty() {
        return Ok(Vec::new());
    }
    let read_len = count.min(data.len());
    let out = data[..read_len].to_vec();
    if consume {
        BLOB_CACHE
            .insert(blob_id, data[read_len..].to_vec())
            .map_err(exofs_to_bridge_error)?;
        let _ = BLOB_CACHE.mark_dirty(&blob_id);
    }
    Ok(out)
}

#[inline]
fn append_pipe_payload(blob_id: BlobId, bytes: &[u8]) -> Result<i64, FsBridgeError> {
    if bytes.is_empty() {
        return Ok(0);
    }
    let mut data = BLOB_CACHE
        .get(&blob_id)
        .map(|existing| existing.to_vec())
        .unwrap_or_default();
    data.try_reserve(bytes.len())
        .map_err(|_| FsBridgeError::NoSpace)?;
    data.extend_from_slice(bytes);
    BLOB_CACHE
        .insert(blob_id, data)
        .map_err(exofs_to_bridge_error)?;
    let _ = BLOB_CACHE.mark_dirty(&blob_id);
    Ok(bytes.len() as i64)
}

#[inline]
fn fd_readiness(fd: u32, pid: u32) -> Result<(bool, bool), FsBridgeError> {
    let resolved = resolve_fd(pid, fd)?;
    if is_tty_handle(resolved.handle) {
        return Ok((
            fd_can_read_flags(resolved.flags),
            fd_can_write_flags(resolved.flags),
        ));
    }
    let entry = OBJECT_TABLE
        .get(resolved.handle)
        .map_err(exofs_to_bridge_error)?;
    let mut readable = entry.can_read();
    let writable = entry.can_write();

    if readable && is_pseudo_blob(&entry.blob_id, PSEUDO_PIPE_TAG) {
        readable = blob_len(&entry.blob_id) != 0;
    } else if readable && is_pseudo_blob(&entry.blob_id, PSEUDO_EVENTFD_TAG) {
        readable = eventfd_state(entry.blob_id)
            .map(|(value, _)| value != 0)
            .unwrap_or(false);
    } else if readable && is_pseudo_blob(&entry.blob_id, PSEUDO_INOTIFY_TAG) {
        readable = false;
    } else if readable && is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG) {
        readable = socket_payload_len(entry.blob_id) != 0;
    }

    Ok((readable, writable))
}

#[inline]
fn set_fdset_bit(set: &mut [u8], fd: usize, present: bool) {
    let byte = fd / 8;
    let bit = fd % 8;
    if byte >= set.len() {
        return;
    }
    let mask = 1u8 << bit;
    if present {
        set[byte] |= mask;
    } else {
        set[byte] &= !mask;
    }
}

#[inline]
fn fdset_bit(set: &[u8], fd: usize) -> bool {
    let byte = fd / 8;
    let bit = fd % 8;
    byte < set.len() && (set[byte] & (1u8 << bit)) != 0
}

#[inline]
fn flock_range_from_abi(fl: LinuxFlock) -> Result<(u64, u64), FsBridgeError> {
    if fl.l_whence != SEEK_SET as i16 || fl.l_start < 0 || fl.l_len < 0 {
        return Err(FsBridgeError::Invalid);
    }
    let start = fl.l_start as u64;
    let len = if fl.l_len == 0 {
        u64::MAX.saturating_sub(start)
    } else {
        fl.l_len as u64
    };
    Ok((start, len))
}

#[inline]
fn statfs_snapshot() -> LinuxStatFs {
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
fn blob_id_to_object_id(blob_id: BlobId) -> ObjectId {
    ObjectId(*blob_id.as_bytes())
}

#[inline]
fn object_id_to_blob_id(object_id: ObjectId) -> BlobId {
    BlobId(*object_id.as_bytes())
}

#[inline]
fn ensure_root_directory() -> Result<(), FsBridgeError> {
    let root_blob = blob_id_for_path(b"/")?;
    if BLOB_CACHE.contains(&root_blob) {
        return Ok(());
    }

    if let Some(data) =
        object_store::load_blob_data_if_available(&root_blob).map_err(exofs_to_bridge_error)?
    {
        if !blob_is_directory(&data) {
            return Err(FsBridgeError::NotDir);
        }
        return BLOB_CACHE
            .insert(root_blob, data)
            .map_err(exofs_to_bridge_error);
    }

    let dir_index = PathIndex::new_with_key(ObjectId::default(), directory_mount_key());
    let bytes = dir_index.serialize().map_err(exofs_to_bridge_error)?;
    BLOB_CACHE
        .insert(root_blob, bytes)
        .map_err(exofs_to_bridge_error)?;
    let _ = BLOB_CACHE.mark_dirty(&root_blob);
    Ok(())
}

#[inline]
fn load_path_index(path: &[u8]) -> Result<PathIndex, FsBridgeError> {
    let blob_id = blob_id_for_path(path)?;
    let data = snapshot_blob(&blob_id)?;
    if !blob_is_directory(&data) {
        return Err(FsBridgeError::NotDir);
    }
    PathIndex::from_bytes(&data).map_err(exofs_to_bridge_error)
}

#[inline]
fn store_path_index(path: &[u8], index: &PathIndex) -> Result<(), FsBridgeError> {
    let blob_id = blob_id_for_path(path)?;
    let bytes = index.serialize().map_err(exofs_to_bridge_error)?;
    BLOB_CACHE
        .insert(blob_id, bytes)
        .map_err(exofs_to_bridge_error)?;
    let _ = BLOB_CACHE.mark_dirty(&blob_id);
    Ok(())
}

#[inline]
fn ensure_directory_exists(path: &[u8]) -> Result<(), FsBridgeError> {
    if path == b"/" {
        return ensure_root_directory();
    }

    let blob_id = blob_id_for_path(path)?;
    let data = snapshot_blob(&blob_id)?;
    if blob_is_directory(&data) {
        Ok(())
    } else {
        Err(FsBridgeError::NotDir)
    }
}

#[inline]
fn ensure_directory_chain(path: &[u8]) -> Result<(), FsBridgeError> {
    let normalized_path = normalized_path_bytes(path)?;
    ensure_root_directory()?;
    if normalized_path == b"/" {
        return Ok(());
    }

    let components = normalize_path_buf(&normalized_path)?;
    let mut current = PathComponentBuf::new();

    for comp in components.iter() {
        let parent_path = path_buf_to_bytes(&current)?;
        let next_comp = comp.clone();
        current
            .push(next_comp.clone())
            .map_err(exofs_to_bridge_error)?;
        let current_path = path_buf_to_bytes(&current)?;

        match path_entry(&current_path) {
            Ok((blob_id, kind)) => {
                if kind == PATH_INDEX_KIND_DIR {
                    continue;
                }

                let data = snapshot_blob(&blob_id)?;
                if !blob_is_directory(&data) {
                    return Err(FsBridgeError::NotDir);
                }
            }
            Err(FsBridgeError::NotFound) => {
                let blob_id = blob_id_for_path(&current_path)?;
                let parent_oid = if parent_path == b"/" {
                    ObjectId::default()
                } else {
                    blob_id_to_object_id(blob_id_for_path(&parent_path)?)
                };

                let dir_index = PathIndex::new_with_key(parent_oid, directory_mount_key());
                let bytes = dir_index.serialize().map_err(exofs_to_bridge_error)?;
                BLOB_CACHE
                    .insert(blob_id, bytes)
                    .map_err(exofs_to_bridge_error)?;
                let _ = BLOB_CACHE.mark_dirty(&blob_id);
                upsert_parent_entry(&parent_path, &next_comp, blob_id, PATH_INDEX_KIND_DIR)?;
            }
            Err(err) => return Err(err),
        }
    }

    Ok(())
}

#[inline]
fn dirent_type_from_kind(kind: u8) -> u8 {
    match kind {
        PATH_INDEX_KIND_DIR => DT_DIR,
        PATH_INDEX_KIND_FILE => DT_REG,
        PATH_INDEX_KIND_SYMLINK => DT_LNK,
        _ => DT_UNKNOWN,
    }
}

#[inline]
fn inode_from_object_id(object_id: &ObjectId) -> u64 {
    let bytes = object_id.as_bytes();
    u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])
}

#[inline]
fn upsert_parent_entry(
    parent_path: &[u8],
    leaf: &PathComponent,
    child_blob_id: BlobId,
    kind: u8,
) -> Result<(), FsBridgeError> {
    ensure_directory_exists(parent_path)?;
    let mut index = load_path_index(parent_path)?;
    if index.lookup(leaf).is_some() {
        index.remove(leaf).map_err(exofs_to_bridge_error)?;
    }
    index
        .insert(leaf, blob_id_to_object_id(child_blob_id), kind)
        .map_err(exofs_to_bridge_error)?;
    store_path_index(parent_path, &index)
}

#[inline]
fn remove_parent_entry(parent_path: &[u8], leaf: &PathComponent) -> Result<(), FsBridgeError> {
    ensure_directory_exists(parent_path)?;
    let mut index = load_path_index(parent_path)?;
    index.remove(leaf).map_err(exofs_to_bridge_error)?;
    store_path_index(parent_path, &index)
}

#[inline]
fn directory_mount_key() -> [u8; 16] {
    #[cfg(target_os = "none")]
    {
        crate::fs::exofs::path::path_index::mount_secret_key()
    }

    #[cfg(not(target_os = "none"))]
    {
        [0xA5, 0, 0, 0, 0, 0, 0, 0x5A, 0, 0, 0, 0, 0, 0, 0, 0xC3]
    }
}

#[inline]
fn path_index_entry_count(data: &[u8]) -> Option<u32> {
    if data.len() < size_of::<PathIndexHeader>() {
        return None;
    }

    let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    if magic != PATH_INDEX_MAGIC {
        return None;
    }

    let version = u16::from_le_bytes([data[4], data[5]]);
    if version != PATH_INDEX_VERSION {
        return None;
    }

    Some(u32::from_le_bytes([data[40], data[41], data[42], data[43]]))
}

#[inline]
fn blob_is_directory(data: &[u8]) -> bool {
    path_index_entry_count(data).is_some()
}

#[inline]
fn blob_is_directory_by_id(blob_id: &BlobId) -> bool {
    snapshot_blob(blob_id)
        .ok()
        .as_deref()
        .and_then(path_index_entry_count)
        .is_some()
}

fn default_stat_mode_for_kind(kind: u8, is_dir: bool) -> u32 {
    match kind {
        PATH_INDEX_KIND_DIR => STAT_MODE_DIR,
        PATH_INDEX_KIND_SYMLINK => STAT_MODE_SYMLINK,
        _ if is_dir => STAT_MODE_DIR,
        _ => STAT_MODE_FILE,
    }
}

fn stat_mode_for_blob(blob_id: &BlobId, kind: u8, is_dir: bool) -> u32 {
    stored_mode(blob_id).unwrap_or_else(|| default_stat_mode_for_kind(kind, is_dir))
}

#[inline]
fn inode_from_blob_id(blob_id: &BlobId) -> u64 {
    let bytes = blob_id.as_bytes();
    u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])
}

#[inline]
fn blocks_for_size(size: u64) -> i64 {
    size.saturating_add(511).saturating_div(512) as i64
}

#[inline]
fn linux_stat_for_blob_meta(
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
fn linux_statx_for_blob_meta(
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
fn linux_statx_from_stat(stat: LinuxStat) -> LinuxStatx {
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
fn path_entry(path: &[u8]) -> Result<(BlobId, u8), FsBridgeError> {
    ensure_root_directory()?;
    if path == b"/" {
        return Ok((blob_id_for_path(path)?, PATH_INDEX_KIND_DIR));
    }
    let (parent_path, leaf) = split_parent_and_leaf(path)?;
    let index = load_path_index(&parent_path)?;
    let (object_id, kind) = index.lookup(&leaf).ok_or(FsBridgeError::NotFound)?;
    Ok((object_id_to_blob_id(object_id), kind))
}

fn resolve_path_with_symlinks(
    path: &[u8],
    follow_last: bool,
    allow_missing_final: bool,
) -> Result<Vec<u8>, FsBridgeError> {
    let mut pending = normalize_path_buf(path)?;
    let mut resolved = PathComponentBuf::new();
    let mut depth = 0usize;
    let mut idx = 0usize;

    ensure_root_directory()?;

    while idx < pending.len() {
        let comp = pending.as_slice()[idx].clone();
        let parent_path = path_buf_to_bytes(&resolved)?;
        ensure_directory_exists(&parent_path)?;

        let index = load_path_index(&parent_path)?;
        let (object_id, kind) = match index.lookup(&comp) {
            Some(entry) => entry,
            None => {
                let is_last = idx + 1 == pending.len();
                if !allow_missing_final || !is_last {
                    return Err(FsBridgeError::NotFound);
                }
                resolved.push(comp).map_err(exofs_to_bridge_error)?;
                return path_buf_to_bytes(&resolved);
            }
        };

        let is_last = idx + 1 == pending.len();
        if kind == PATH_INDEX_KIND_SYMLINK && (!is_last || follow_last) {
            depth = depth.saturating_add(1);
            if depth > SYMLINK_MAX_DEPTH {
                return Err(FsBridgeError::Loop);
            }

            let raw_target = snapshot_blob(&object_id_to_blob_id(object_id))?;
            if !is_valid_symlink_target(&raw_target) {
                return Err(FsBridgeError::Invalid);
            }

            let mut next = if raw_target.starts_with(b"/") {
                normalize_path_buf(&raw_target)?
            } else {
                let mut joined = resolved.clone();
                let target_buf =
                    PathComponentBuf::from_path(&raw_target).map_err(exofs_to_bridge_error)?;
                joined
                    .extend_from(&target_buf)
                    .map_err(exofs_to_bridge_error)?;
                joined
            };

            let mut rem_idx = idx + 1;
            while rem_idx < pending.len() {
                next.push(pending.as_slice()[rem_idx].clone())
                    .map_err(exofs_to_bridge_error)?;
                rem_idx += 1;
            }
            next.normalize().map_err(exofs_to_bridge_error)?;
            pending = next;
            resolved = PathComponentBuf::new();
            idx = 0;
            continue;
        }

        resolved.push(comp).map_err(exofs_to_bridge_error)?;
        idx += 1;
    }

    path_buf_to_bytes(&resolved)
}

/// `read(fd, buf, count)` → octets lus.
/// PONT : `crate::fs::vfs::read(fd, buf_slice)` — activé quand `pub mod fs;`
#[inline]
pub fn fs_read(fd: u32, buf_ptr: u64, count: usize, pid: u32) -> Result<i64, FsBridgeError> {
    if buf_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    if count == 0 {
        return Ok(0);
    }
    let resolved = resolve_fd(pid, fd)?;
    if is_tty_handle(resolved.handle) {
        if !fd_can_read_flags(resolved.flags) {
            return Err(FsBridgeError::BadFd);
        }
        return tty_read_bytes(buf_ptr, count, pid);
    }
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }

    let obj_fd = resolved.handle;
    let entry = OBJECT_TABLE.get(obj_fd).map_err(exofs_to_bridge_error)?;
    if !entry.can_read() {
        return Err(FsBridgeError::PermDenied);
    }

    if is_pseudo_blob(&entry.blob_id, PSEUDO_PIPE_TAG) {
        let data = BLOB_CACHE
            .get(&entry.blob_id)
            .map(|bytes| bytes.to_vec())
            .unwrap_or_default();
        if data.is_empty() {
            return Err(FsBridgeError::WouldBlock);
        }
        let read_len = count.min(data.len());
        copy_to_user(buf_ptr as *mut u8, data.as_ptr(), read_len)
            .map_err(|_| FsBridgeError::Fault)?;
        let remaining = data[read_len..].to_vec();
        BLOB_CACHE
            .insert(entry.blob_id, remaining)
            .map_err(exofs_to_bridge_error)?;
        let _ = BLOB_CACHE.mark_dirty(&entry.blob_id);
        return Ok(read_len as i64);
    }

    if is_pseudo_blob(&entry.blob_id, PSEUDO_EVENTFD_TAG) {
        if count < size_of::<u64>() {
            return Err(FsBridgeError::Invalid);
        }
        let (value, flags) = eventfd_state(entry.blob_id)?;
        if value == 0 {
            return Err(FsBridgeError::WouldBlock);
        }
        let out = if flags & EFD_SEMAPHORE != 0 { 1 } else { value };
        let next = if flags & EFD_SEMAPHORE != 0 {
            value.saturating_sub(1)
        } else {
            0
        };
        write_user_typed(buf_ptr, out).map_err(|_| FsBridgeError::Fault)?;
        store_eventfd_state(entry.blob_id, next, flags)?;
        return Ok(size_of::<u64>() as i64);
    }

    if is_pseudo_blob(&entry.blob_id, PSEUDO_INOTIFY_TAG) {
        return Err(FsBridgeError::WouldBlock);
    }

    if is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG) {
        let data = read_socket_payload(entry.blob_id, count, true)?;
        if data.is_empty() {
            return Err(FsBridgeError::WouldBlock);
        }
        copy_to_user(buf_ptr as *mut u8, data.as_ptr(), data.len())
            .map_err(|_| FsBridgeError::Fault)?;
        return Ok(data.len() as i64);
    }

    let start = entry.cursor as usize;
    let data = match BLOB_CACHE.read_at(&entry.blob_id, start, count) {
        Ok(data) => data,
        Err(ExofsError::BlobNotFound) if entry.size == 0 => return Ok(0),
        Err(err) => return Err(exofs_to_bridge_error(err)),
    };
    if data.is_empty() {
        return Ok(0);
    }
    let read_len = data.len();
    copy_to_user(buf_ptr as *mut u8, data.as_ptr(), read_len).map_err(|_| FsBridgeError::Fault)?;
    OBJECT_TABLE
        .advance_cursor(obj_fd, read_len as u64)
        .map_err(exofs_to_bridge_error)?;
    Ok(read_len as i64)
}

/// `write(fd, buf, count)` → octets écrits.
#[inline]
pub fn fs_write(fd: u32, buf_ptr: u64, count: usize, pid: u32) -> Result<i64, FsBridgeError> {
    if buf_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    if count == 0 {
        return Ok(0);
    }
    let resolved = resolve_fd(pid, fd)?;
    if is_tty_handle(resolved.handle) {
        if !fd_can_write_flags(resolved.flags) {
            return Err(FsBridgeError::BadFd);
        }
        let input = read_user_bytes(buf_ptr, count)?;
        return tty_write_bytes(pid, &input);
    }
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }

    let obj_fd = resolved.handle;
    let entry = OBJECT_TABLE.get(obj_fd).map_err(exofs_to_bridge_error)?;
    if !entry.can_write() {
        return Err(FsBridgeError::PermDenied);
    }

    if is_pseudo_blob(&entry.blob_id, PSEUDO_PIPE_TAG) {
        if OBJECT_TABLE.readable_count_for(&entry.blob_id) == 0 {
            let _ = crate::process::signal::delivery::send_signal_to_pid(
                crate::process::core::pid::Pid(pid),
                crate::process::signal::default::Signal::SIGPIPE,
            );
            return Err(FsBridgeError::BrokenPipe);
        }
        let input = read_user_bytes(buf_ptr, count)?;
        let mut data = BLOB_CACHE
            .get(&entry.blob_id)
            .map(|bytes| bytes.to_vec())
            .unwrap_or_default();
        data.try_reserve(input.len())
            .map_err(|_| FsBridgeError::NoSpace)?;
        data.extend_from_slice(&input);
        BLOB_CACHE
            .insert(entry.blob_id, data)
            .map_err(exofs_to_bridge_error)?;
        let _ = BLOB_CACHE.mark_dirty(&entry.blob_id);
        return Ok(count as i64);
    }

    if is_pseudo_blob(&entry.blob_id, PSEUDO_EVENTFD_TAG) {
        if count < size_of::<u64>() {
            return Err(FsBridgeError::Invalid);
        }
        let add = read_user_typed::<u64>(buf_ptr).map_err(|_| FsBridgeError::Fault)?;
        if add == u64::MAX {
            return Err(FsBridgeError::Invalid);
        }
        let (value, flags) = eventfd_state(entry.blob_id)?;
        let next = value.checked_add(add).ok_or(FsBridgeError::WouldBlock)?;
        store_eventfd_state(entry.blob_id, next, flags)?;
        return Ok(size_of::<u64>() as i64);
    }

    if is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG) {
        let input = read_user_bytes(buf_ptr, count)?;
        let peer = socket_peer_blob(entry.blob_id)?;
        return append_socket_payload(peer, &input);
    }

    let input = read_user_bytes(buf_ptr, count)?;

    let start = if entry.flags & open_flags::O_APPEND != 0 {
        blob_len(&entry.blob_id)
    } else {
        entry.cursor as usize
    };
    let end = start.checked_add(count).ok_or(FsBridgeError::NoSpace)?;
    BLOB_CACHE
        .write_at(entry.blob_id, start, &input)
        .map_err(exofs_to_bridge_error)?;
    OBJECT_TABLE
        .set_cursor(obj_fd, end as u64)
        .map_err(exofs_to_bridge_error)?;
    OBJECT_TABLE
        .set_size(obj_fd, end as u64)
        .map_err(exofs_to_bridge_error)?;
    let _ = pid;
    Ok(count as i64)
}

/// `pread64(fd, buf, count, offset)` without moving the shared file cursor.
#[inline]
pub fn fs_pread64(
    fd: u32,
    buf_ptr: u64,
    count: usize,
    offset: u64,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    let obj_fd = resolve_fd(pid, fd)?.handle;
    let entry = OBJECT_TABLE.get(obj_fd).map_err(exofs_to_bridge_error)?;
    if !entry.can_read() {
        return Err(FsBridgeError::PermDenied);
    }
    read_blob_at(entry.blob_id, offset, buf_ptr, count)
}

/// `pwrite64(fd, buf, count, offset)` without moving the shared file cursor.
#[inline]
pub fn fs_pwrite64(
    fd: u32,
    buf_ptr: u64,
    count: usize,
    offset: u64,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    let obj_fd = resolve_fd(pid, fd)?.handle;
    let entry = OBJECT_TABLE.get(obj_fd).map_err(exofs_to_bridge_error)?;
    if !entry.can_write() {
        return Err(FsBridgeError::PermDenied);
    }
    let written = write_blob_at(entry.blob_id, offset, buf_ptr, count)?;
    let new_size = offset.saturating_add(written as u64).max(entry.size);
    OBJECT_TABLE
        .set_size(fd, new_size)
        .map_err(exofs_to_bridge_error)?;
    Ok(written)
}

#[inline]
fn vectored_io<F>(iov_ptr: u64, iovcnt: u32, mut op: F) -> Result<i64, FsBridgeError>
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

/// `readv(fd, iov, iovcnt)`.
#[inline]
pub fn fs_readv(fd: u32, iov_ptr: u64, iovcnt: u32, pid: u32) -> Result<i64, FsBridgeError> {
    vectored_io(iov_ptr, iovcnt, |base, len| fs_read(fd, base, len, pid))
}

/// `writev(fd, iov, iovcnt)`.
#[inline]
pub fn fs_writev(fd: u32, iov_ptr: u64, iovcnt: u32, pid: u32) -> Result<i64, FsBridgeError> {
    vectored_io(iov_ptr, iovcnt, |base, len| fs_write(fd, base, len, pid))
}

/// `preadv(fd, iov, iovcnt, offset)`.
#[inline]
pub fn fs_preadv(
    fd: u32,
    iov_ptr: u64,
    iovcnt: u32,
    offset: u64,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    let mut pos = offset;
    vectored_io(iov_ptr, iovcnt, |base, len| {
        let n = fs_pread64(fd, base, len, pos, pid)?;
        pos = pos.saturating_add(n as u64);
        Ok(n)
    })
}

/// `pwritev(fd, iov, iovcnt, offset)`.
#[inline]
pub fn fs_pwritev(
    fd: u32,
    iov_ptr: u64,
    iovcnt: u32,
    offset: u64,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    let mut pos = offset;
    vectored_io(iov_ptr, iovcnt, |base, len| {
        let n = fs_pwrite64(fd, base, len, pos, pid)?;
        pos = pos.saturating_add(n as u64);
        Ok(n)
    })
}

/// `open(path, flags, mode)` → fd.
#[inline]
pub fn fs_open(path: &[u8], flags: u32, mode: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if is_tty_path(path) {
        let fd_flags = fd_table_flags(flags, open_flags::O_RDWR);
        if let Some(logical_fd) = install_process_fd(pid, TTY_PTS0_HANDLE as u64, fd_flags) {
            return Ok(logical_fd as i64);
        }
        return Ok(TTY_PTS0_HANDLE as i64);
    }
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let fd_flags = flags & !(O_CLOEXEC | O_NONBLOCK);
    if !open_flags::validate(fd_flags) {
        return Err(FsBridgeError::Invalid);
    }

    ensure_root_directory()?;
    let normalized_input = normalized_path_bytes(path)?;
    if fd_flags & open_flags::O_CREAT != 0 && normalized_input != b"/" {
        let (parent_path, _) = split_parent_and_leaf(&normalized_input)?;
        ensure_directory_chain(&parent_path)?;
    }
    let normalized_path = resolve_path_with_symlinks(path, true, true)?;
    let existing_entry = path_entry(&normalized_path).ok();
    let blob_id = match existing_entry {
        Some((existing_blob_id, _)) => existing_blob_id,
        None => blob_id_for_path(&normalized_path)?,
    };
    let exists = existing_entry.is_some();

    if !exists && fd_flags & open_flags::O_CREAT == 0 {
        return Err(FsBridgeError::NotFound);
    }
    if exists && (fd_flags & open_flags::O_CREAT != 0) && (fd_flags & open_flags::O_EXCL != 0) {
        return Err(FsBridgeError::Exists);
    }
    if !exists {
        let effective_mode = S_IFREG | apply_umask(mode, 0o666, pid);
        let (parent_path, leaf) = split_parent_and_leaf(&normalized_path)?;
        ensure_directory_exists(&parent_path)?;
        BLOB_CACHE
            .insert(blob_id, Vec::new())
            .map_err(exofs_to_bridge_error)?;
        let _ = BLOB_CACHE.mark_dirty(&blob_id);
        upsert_mode(blob_id, effective_mode);
        upsert_parent_entry(&parent_path, &leaf, blob_id, PATH_INDEX_KIND_FILE)?;
    }
    if exists {
        ensure_blob_exists(blob_id)?;
    }
    if fd_flags & open_flags::O_TRUNC != 0 {
        if !open_flags::can_write(fd_flags) {
            return Err(FsBridgeError::Invalid);
        }
        if existing_entry
            .map(|(_, kind)| kind == PATH_INDEX_KIND_DIR)
            .unwrap_or(false)
        {
            return Err(FsBridgeError::IsDir);
        }
        BLOB_CACHE
            .insert(blob_id, Vec::new())
            .map_err(exofs_to_bridge_error)?;
        let _ = BLOB_CACHE.mark_dirty(&blob_id);
    }

    let size = blob_len(&blob_id) as u64;
    let fd = OBJECT_TABLE
        .open(blob_id, fd_flags, size, 0, pid as u64)
        .map_err(exofs_to_bridge_error)?;
    if fd_flags & open_flags::O_APPEND != 0 {
        OBJECT_TABLE
            .set_cursor(fd, size)
            .map_err(exofs_to_bridge_error)?;
    }
    if let Some(logical_fd) = install_process_fd(pid, fd as u64, fd_table_flags(flags, fd_flags)) {
        Ok(logical_fd as i64)
    } else {
        Ok(fd as i64)
    }
}

/// `close(fd)`.
#[inline]
pub fn fs_close(fd: u32, pid: u32) -> Result<i64, FsBridgeError> {
    let resolved = resolve_fd(pid, fd)?;
    let handle = close_process_fd(pid, fd).unwrap_or(resolved.handle as u64);
    if is_tty_handle_u64(handle) {
        return Ok(0);
    }
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if handle > u32::MAX as u64 {
        return Err(FsBridgeError::BadFd);
    }
    let obj_fd = handle as u32;
    let entry = OBJECT_TABLE.get(obj_fd).map_err(exofs_to_bridge_error)?;
    if pid != 0 && entry.owner_uid != 0 && entry.owner_uid != pid as u64 {
        return Err(FsBridgeError::BadFd);
    }
    if OBJECT_TABLE.close(obj_fd) {
        Ok(0)
    } else {
        Err(FsBridgeError::BadFd)
    }
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

/// `lseek(fd, offset, whence)` → nouvelle position.
#[inline]
pub fn fs_lseek(fd: u32, offset: i64, whence: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let obj_fd = resolve_fd(pid, fd)?.handle;
    let entry = OBJECT_TABLE.get(obj_fd).map_err(exofs_to_bridge_error)?;
    let base = match whence {
        SEEK_SET => 0i64,
        SEEK_CUR => entry.cursor as i64,
        SEEK_END => blob_len(&entry.blob_id) as i64,
        _ => return Err(FsBridgeError::Invalid),
    };
    let new_pos = base.checked_add(offset).ok_or(FsBridgeError::Invalid)?;
    if new_pos < 0 {
        return Err(FsBridgeError::Invalid);
    }
    OBJECT_TABLE
        .set_cursor(obj_fd, new_pos as u64)
        .map_err(exofs_to_bridge_error)?;
    Ok(new_pos)
}

/// `stat(path, stat_ptr)`.
#[inline]
pub fn fs_stat(path: &[u8], stat_ptr: u64, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if stat_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    let normalized_path = resolve_path_with_symlinks(path, true, false)?;
    let (blob_id, kind) = path_entry(&normalized_path)?;
    let size = blob_len(&blob_id) as u64;
    let is_dir = kind == PATH_INDEX_KIND_DIR;
    let stat = linux_stat_for_blob_meta(blob_id, size, pid, kind, is_dir);
    write_user_typed(stat_ptr, stat).map_err(|_| FsBridgeError::Fault)?;
    Ok(0)
}

/// `lstat(path, stat_ptr)` — ne suit pas le symlink terminal.
#[inline]
pub fn fs_lstat(path: &[u8], stat_ptr: u64, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if stat_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    let normalized_path = resolve_path_with_symlinks(path, false, false)?;
    let (blob_id, kind) = path_entry(&normalized_path)?;
    let stat = linux_stat_for_blob_meta(
        blob_id,
        blob_len(&blob_id) as u64,
        pid,
        kind,
        kind == PATH_INDEX_KIND_DIR,
    );
    write_user_typed(stat_ptr, stat).map_err(|_| FsBridgeError::Fault)?;
    Ok(0)
}

/// `fstat(fd, stat_ptr)`.
#[inline]
pub fn fs_fstat(fd: u32, stat_ptr: u64, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if stat_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    let obj_fd = resolve_fd(pid, fd)?.handle;
    let entry = OBJECT_TABLE.get(obj_fd).map_err(exofs_to_bridge_error)?;
    let owner_uid = if entry.owner_uid == 0 {
        pid
    } else {
        entry.owner_uid as u32
    };
    let is_dir = blob_is_directory_by_id(&entry.blob_id);
    let kind = if is_dir {
        PATH_INDEX_KIND_DIR
    } else {
        PATH_INDEX_KIND_FILE
    };
    let stat = linux_stat_for_blob_meta(
        entry.blob_id,
        blob_len(&entry.blob_id) as u64,
        owner_uid,
        kind,
        is_dir,
    );
    write_user_typed(stat_ptr, stat).map_err(|_| FsBridgeError::Fault)?;
    Ok(0)
}

/// `openat(dirfd, path, flags, mode)`.
#[inline]
pub fn fs_openat(
    dirfd: i32,
    path: &[u8],
    flags: u32,
    mode: u32,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    // ExoFS ne maintient pas encore de namespace de répertoires ouverts.
    // Cas supportés sans ambiguïté:
    // - AT_FDCWD + chemin relatif/absolu
    // - chemin absolu, quel que soit dirfd (sémantique POSIX)
    if dirfd == AT_FDCWD || path.starts_with(b"/") {
        return fs_open(path, flags, mode, pid);
    }
    Err(FsBridgeError::Invalid)
}

/// `symlink(target, linkpath)`.
#[inline]
pub fn fs_symlink(target: &[u8], linkpath: &[u8], pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    if !is_valid_symlink_target(target) {
        return Err(FsBridgeError::Invalid);
    }

    let normalized_link = resolve_path_with_symlinks(linkpath, false, true)?;
    if normalized_link == b"/" {
        return Err(FsBridgeError::BadPath);
    }
    if path_entry(&normalized_link).is_ok() {
        return Err(FsBridgeError::Exists);
    }

    let (parent_path, leaf) = split_parent_and_leaf(&normalized_link)?;
    ensure_directory_exists(&parent_path)?;
    let blob_id = blob_id_for_path(&normalized_link)?;
    BLOB_CACHE
        .insert(blob_id, target.to_vec())
        .map_err(exofs_to_bridge_error)?;
    let _ = BLOB_CACHE.mark_dirty(&blob_id);
    register_symlink(&blob_id_to_object_id(blob_id), target).map_err(exofs_to_bridge_error)?;
    upsert_parent_entry(&parent_path, &leaf, blob_id, PATH_INDEX_KIND_SYMLINK)?;
    Ok(0)
}

/// `symlinkat(target, dirfd, linkpath)`.
#[inline]
pub fn fs_symlinkat(
    target: &[u8],
    dirfd: i32,
    linkpath: &[u8],
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if dirfd == AT_FDCWD || linkpath.starts_with(b"/") {
        return fs_symlink(target, linkpath, pid);
    }
    Err(FsBridgeError::Invalid)
}

/// `dup(oldfd)`.
#[inline]
pub fn fs_dup(oldfd: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let resolved = resolve_fd(pid, oldfd)?;
    if is_tty_handle(resolved.handle) {
        if let Some(fd) = install_process_fd(pid, TTY_PTS0_HANDLE as u64, resolved.flags) {
            return Ok(fd as i64);
        }
        return Err(FsBridgeError::NoMemory);
    }
    let handle = OBJECT_TABLE
        .dup(resolved.handle)
        .map_err(exofs_to_bridge_error)?;
    if let Some(fd) = install_process_fd(pid, handle as u64, resolved.flags) {
        Ok(fd as i64)
    } else {
        Ok(handle as i64)
    }
}

/// `dup2(oldfd, newfd)`.
#[inline]
pub fn fs_dup2(oldfd: u32, newfd: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if oldfd == newfd {
        let _ = resolve_fd(pid, oldfd)?;
        return Ok(newfd as i64);
    }
    let resolved = resolve_fd(pid, oldfd)?;
    if let Some(old_handle) = close_process_fd(pid, newfd) {
        if !is_tty_handle_u64(old_handle) && old_handle <= u32::MAX as u64 {
            let _ = OBJECT_TABLE.close(old_handle as u32);
        }
    }
    if is_tty_handle(resolved.handle) {
        if install_process_fd_at(pid, newfd, TTY_PTS0_HANDLE as u64, resolved.flags) {
            return Ok(newfd as i64);
        }
        return Err(FsBridgeError::NoMemory);
    }
    let handle = OBJECT_TABLE
        .dup(resolved.handle)
        .map_err(exofs_to_bridge_error)?;
    if install_process_fd_at(pid, newfd, handle as u64, resolved.flags) {
        Ok(newfd as i64)
    } else {
        OBJECT_TABLE
            .dup2(resolved.handle, newfd)
            .map(|fd| fd as i64)
            .map_err(exofs_to_bridge_error)
    }
}

/// `fcntl(fd, cmd, arg)`.
#[inline]
pub fn fs_fcntl(fd: u32, cmd: u32, arg: u64, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    match cmd {
        F_DUPFD => {
            let resolved = resolve_fd(pid, fd)?;
            if arg > i32::MAX as u64 {
                return Err(FsBridgeError::Invalid);
            }
            let min_fd = arg as i32;
            if is_tty_handle(resolved.handle) {
                if let Some(pcb) = crate::process::core::registry::PROCESS_REGISTRY
                    .find_by_pid(crate::process::core::pid::Pid(pid))
                {
                    let mut files = pcb.files.lock();
                    let mut candidate = min_fd.max(0);
                    while candidate < 65536 {
                        if files.get(candidate).is_none() {
                            if files.install_at(candidate, TTY_PTS0_HANDLE as u64, resolved.flags) {
                                return Ok(candidate as i64);
                            }
                            return Err(FsBridgeError::NoMemory);
                        }
                        candidate += 1;
                    }
                    return Err(FsBridgeError::NoSpace);
                }
                return Err(FsBridgeError::BadFd);
            }
            let handle = OBJECT_TABLE
                .dup(resolved.handle)
                .map_err(exofs_to_bridge_error)?;
            if let Some(pcb) = crate::process::core::registry::PROCESS_REGISTRY
                .find_by_pid(crate::process::core::pid::Pid(pid))
            {
                let mut files = pcb.files.lock();
                let mut candidate = min_fd.max(0);
                while candidate < 65536 {
                    if files.get(candidate).is_none() {
                        if files.install_at(candidate, handle as u64, resolved.flags) {
                            return Ok(candidate as i64);
                        }
                        let _ = OBJECT_TABLE.close(handle);
                        return Err(FsBridgeError::NoMemory);
                    }
                    candidate += 1;
                }
                let _ = OBJECT_TABLE.close(handle);
                Err(FsBridgeError::NoSpace)
            } else {
                OBJECT_TABLE
                    .dup_from(resolved.handle, arg as u32)
                    .map(|new_fd| new_fd as i64)
                    .map_err(exofs_to_bridge_error)
            }
        }
        F_GETFD => {
            let _ = resolve_fd(pid, fd)?;
            let flags = process_fd_descriptor(pid, fd)
                .map(|desc| desc.flags)
                .unwrap_or(0);
            Ok(if flags & O_CLOEXEC != 0 {
                FD_CLOEXEC as i64
            } else {
                0
            })
        }
        F_SETFD => {
            if arg & !FD_CLOEXEC != 0 {
                return Err(FsBridgeError::Invalid);
            }
            let mut flags = process_fd_descriptor(pid, fd)
                .map(|desc| desc.flags)
                .unwrap_or(0);
            if arg & FD_CLOEXEC != 0 {
                flags |= O_CLOEXEC;
            } else {
                flags &= !O_CLOEXEC;
            }
            let _ = set_process_fd_flags(pid, fd, flags);
            Ok(0)
        }
        F_GETFL => {
            let resolved = resolve_fd(pid, fd)?;
            if is_tty_handle(resolved.handle) {
                Ok(resolved.flags as i64)
            } else {
                OBJECT_TABLE
                    .get(resolved.handle)
                    .map(|entry| (entry.flags | (resolved.flags & O_NONBLOCK)) as i64)
                    .map_err(exofs_to_bridge_error)
            }
        }
        F_SETFL => {
            let supported = (open_flags::O_APPEND | O_NONBLOCK) as u64;
            if arg & !supported != 0 {
                return Err(FsBridgeError::Invalid);
            }
            let resolved = resolve_fd(pid, fd)?;
            if is_tty_handle(resolved.handle) {
                let access = resolved.flags & 0x3;
                let flags = access | ((arg as u32) & O_NONBLOCK);
                let _ = set_process_fd_flags(pid, fd, flags);
                Ok(flags as i64)
            } else {
                let flags = OBJECT_TABLE
                    .set_status_flags(resolved.handle, (arg as u32) & !O_NONBLOCK)
                    .map_err(exofs_to_bridge_error)?;
                let descriptor_flags = (resolved.flags & (0x3 | O_CLOEXEC))
                    | (flags & open_flags::O_APPEND)
                    | ((arg as u32) & O_NONBLOCK);
                let _ = set_process_fd_flags(pid, fd, descriptor_flags);
                Ok((flags | (descriptor_flags & O_NONBLOCK)) as i64)
            }
        }
        F_SETLK | F_SETLKW => {
            let fl = read_user_typed::<LinuxFlock>(arg).map_err(|_| FsBridgeError::Fault)?;
            let entry = OBJECT_TABLE
                .get(resolve_fd(pid, fd)?.handle)
                .map_err(exofs_to_bridge_error)?;
            let (start, length) = flock_range_from_abi(fl)?;
            let object_id = inode_from_blob_id(&entry.blob_id);
            use crate::fs::exofs::posix_bridge::fcntl_lock::{
                make_lock, validate_lock, LockKind, FCNTL_LOCK_TABLE,
            };
            let kind = match fl.l_type {
                F_RDLCK => LockKind::Read,
                F_WRLCK => LockKind::Write,
                F_UNLCK => LockKind::Unlock,
                _ => return Err(FsBridgeError::Invalid),
            };
            let lock = make_lock(object_id, pid as u64, 0, start, length, kind);
            if kind != LockKind::Unlock {
                validate_lock(&lock).map_err(exofs_to_bridge_error)?;
            }
            FCNTL_LOCK_TABLE
                .acquire(lock)
                .map(|_| 0)
                .map_err(exofs_to_bridge_error)
        }
        F_GETLK => {
            let mut fl = read_user_typed::<LinuxFlock>(arg).map_err(|_| FsBridgeError::Fault)?;
            let entry = OBJECT_TABLE
                .get(resolve_fd(pid, fd)?.handle)
                .map_err(exofs_to_bridge_error)?;
            let (start, length) = flock_range_from_abi(fl)?;
            let object_id = inode_from_blob_id(&entry.blob_id);
            use crate::fs::exofs::posix_bridge::fcntl_lock::{
                make_lock, LockKind, FCNTL_LOCK_TABLE,
            };
            let kind = match fl.l_type {
                F_RDLCK => LockKind::Read,
                F_WRLCK => LockKind::Write,
                F_UNLCK => LockKind::Unlock,
                _ => return Err(FsBridgeError::Invalid),
            };
            let candidate = make_lock(object_id, pid as u64, 0, start, length, kind);
            match FCNTL_LOCK_TABLE
                .test_lock(&candidate)
                .map_err(exofs_to_bridge_error)?
            {
                Some(info) => {
                    fl.l_type = if info.kind == LockKind::Read as u8 {
                        F_RDLCK
                    } else {
                        F_WRLCK
                    };
                    fl.l_whence = SEEK_SET as i16;
                    fl.l_start = info.start as i64;
                    fl.l_len = if info.length == u64::MAX {
                        0
                    } else {
                        info.length as i64
                    };
                    fl.l_pid = info.conflicting_pid as i32;
                }
                None => {
                    fl.l_type = F_UNLCK;
                    fl.l_whence = SEEK_SET as i16;
                    fl.l_start = 0;
                    fl.l_len = 0;
                    fl.l_pid = 0;
                }
            }
            write_user_typed(arg, fl).map_err(|_| FsBridgeError::Fault)?;
            Ok(0)
        }
        _ => Err(FsBridgeError::Invalid),
    }
}

/// `flock(fd, operation)` translated to whole-file POSIX byte-range locks.
#[inline]
pub fn fs_flock(fd: u32, operation: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let clean = operation & !LOCK_NB;
    let entry = OBJECT_TABLE
        .get(resolve_fd(pid, fd)?.handle)
        .map_err(exofs_to_bridge_error)?;
    let kind = match clean {
        LOCK_SH => crate::fs::exofs::posix_bridge::fcntl_lock::LockKind::Read,
        LOCK_EX => crate::fs::exofs::posix_bridge::fcntl_lock::LockKind::Write,
        LOCK_UN => crate::fs::exofs::posix_bridge::fcntl_lock::LockKind::Unlock,
        _ => return Err(FsBridgeError::Invalid),
    };
    let object_id = inode_from_blob_id(&entry.blob_id);
    use crate::fs::exofs::posix_bridge::fcntl_lock::{make_lock, validate_lock, FCNTL_LOCK_TABLE};
    let lock = make_lock(object_id, pid as u64, 0, 0, u64::MAX, kind);
    if kind != crate::fs::exofs::posix_bridge::fcntl_lock::LockKind::Unlock {
        validate_lock(&lock).map_err(exofs_to_bridge_error)?;
    }
    FCNTL_LOCK_TABLE
        .acquire(lock)
        .map(|_| 0)
        .map_err(exofs_to_bridge_error)
}

/// `mkdir(path, mode)`.
#[inline(never)]
pub fn fs_mkdir(path: &[u8], mode: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let normalized_path = normalized_path_bytes(path)?;
    if normalized_path == b"/" {
        ensure_root_directory()?;
        return Ok(0);
    }
    ensure_root_directory()?;
    let (parent_path, leaf) = split_parent_and_leaf(&normalized_path)?;
    ensure_directory_chain(&parent_path)?;
    let blob_id = blob_id_for_path(&normalized_path)?;
    if BLOB_CACHE.contains(&blob_id) {
        return Err(FsBridgeError::Exists);
    }
    let effective_mode = S_IFDIR | apply_umask(mode, 0o777, pid);

    let parent_oid = if parent_path == b"/" {
        ObjectId::default()
    } else {
        blob_id_to_object_id(blob_id_for_path(&parent_path)?)
    };
    let dir_index = PathIndex::new_with_key(parent_oid, directory_mount_key());
    let bytes = dir_index.serialize().map_err(exofs_to_bridge_error)?;
    BLOB_CACHE
        .insert(blob_id, bytes)
        .map_err(exofs_to_bridge_error)?;
    let _ = BLOB_CACHE.mark_dirty(&blob_id);
    upsert_mode(blob_id, effective_mode);
    upsert_parent_entry(&parent_path, &leaf, blob_id, PATH_INDEX_KIND_DIR)?;
    Ok(0)
}

/// `rmdir(path)`.
#[inline]
pub fn fs_rmdir(path: &[u8], pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    let normalized_path = normalized_path_bytes(path)?;
    if normalized_path == b"/" {
        return Err(FsBridgeError::PermDenied);
    }
    let (parent_path, leaf) = split_parent_and_leaf(&normalized_path)?;
    let blob_id = blob_id_for_path(&normalized_path)?;
    let data = snapshot_blob(&blob_id)?;
    let entry_count = path_index_entry_count(&data).ok_or(FsBridgeError::NotDir)?;
    if entry_count != 0 {
        return Err(FsBridgeError::NotEmpty);
    }
    if OBJECT_TABLE.open_count_for(&blob_id) != 0 {
        return Err(FsBridgeError::PermDenied);
    }
    BLOB_CACHE.invalidate(&blob_id);
    remove_parent_entry(&parent_path, &leaf)?;
    Ok(0)
}

/// `unlink(path)`.
#[inline]
pub fn fs_unlink(path: &[u8], pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    let normalized_path = resolve_path_with_symlinks(path, false, false)?;
    if normalized_path == b"/" {
        return Err(FsBridgeError::PermDenied);
    }
    let (parent_path, leaf) = split_parent_and_leaf(&normalized_path)?;
    let (blob_id, kind) = path_entry(&normalized_path)?;
    let data = snapshot_blob(&blob_id)?;
    if kind == PATH_INDEX_KIND_DIR || blob_is_directory(&data) {
        return Err(FsBridgeError::IsDir);
    }
    if OBJECT_TABLE.open_count_for(&blob_id) != 0 {
        return Err(FsBridgeError::PermDenied);
    }
    if kind == PATH_INDEX_KIND_SYMLINK {
        invalidate_symlink(&blob_id_to_object_id(blob_id));
    }
    BLOB_CACHE.invalidate(&blob_id);
    remove_parent_entry(&parent_path, &leaf)?;
    Ok(0)
}

/// `rename(oldpath, newpath)`.
#[inline]
pub fn fs_rename(old_path: &[u8], new_path: &[u8], pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    let old_normalized = resolve_path_with_symlinks(old_path, false, false)?;
    let new_normalized = resolve_path_with_symlinks(new_path, false, true)?;
    if old_normalized == new_normalized {
        return Ok(0);
    }
    if old_normalized == b"/" || new_normalized == b"/" {
        return Err(FsBridgeError::PermDenied);
    }

    let (old_parent, old_leaf) = split_parent_and_leaf(&old_normalized)?;
    let (new_parent, new_leaf) = split_parent_and_leaf(&new_normalized)?;
    ensure_directory_exists(&new_parent)?;

    let (src_blob_id, src_kind) = path_entry(&old_normalized)?;
    let src_data = snapshot_blob(&src_blob_id)?;
    let src_is_dir = src_kind == PATH_INDEX_KIND_DIR || blob_is_directory(&src_data);
    if src_is_dir
        && new_normalized.len() > old_normalized.len()
        && new_normalized.starts_with(&old_normalized)
        && new_normalized[old_normalized.len()] == b'/'
    {
        return Err(FsBridgeError::Invalid);
    }

    match path_entry(&new_normalized) {
        Ok((dst_blob_id, dst_kind)) => {
            let dst_data = snapshot_blob(&dst_blob_id)?;
            let dst_is_dir = dst_kind == PATH_INDEX_KIND_DIR || blob_is_directory(&dst_data);
            if src_is_dir != dst_is_dir {
                return if src_is_dir {
                    Err(FsBridgeError::NotDir)
                } else {
                    Err(FsBridgeError::IsDir)
                };
            }
            if dst_is_dir && path_index_entry_count(&dst_data).unwrap_or(0) != 0 {
                return Err(FsBridgeError::NotEmpty);
            }
            if OBJECT_TABLE.open_count_for(&dst_blob_id) != 0 {
                return Err(FsBridgeError::PermDenied);
            }
            remove_parent_entry(&new_parent, &new_leaf)?;
            BLOB_CACHE.invalidate(&dst_blob_id);
        }
        Err(FsBridgeError::NotFound) => {}
        Err(err) => return Err(err),
    }

    upsert_parent_entry(&new_parent, &new_leaf, src_blob_id, src_kind)?;
    remove_parent_entry(&old_parent, &old_leaf)?;
    let _ = BLOB_CACHE.mark_dirty(&src_blob_id);
    Ok(0)
}

/// `renameat(olddirfd, oldpath, newdirfd, newpath)`.
#[inline]
pub fn fs_renameat(
    olddirfd: i32,
    old_path: &[u8],
    newdirfd: i32,
    new_path: &[u8],
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if olddirfd == AT_FDCWD || old_path.starts_with(b"/") {
        if newdirfd == AT_FDCWD || new_path.starts_with(b"/") {
            return fs_rename(old_path, new_path, pid);
        }
    }
    Err(FsBridgeError::Invalid)
}

#[inline]
fn fs_link_with_follow(
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

/// `link(oldpath, newpath)`.
#[inline]
pub fn fs_link(old_path: &[u8], new_path: &[u8], pid: u32) -> Result<i64, FsBridgeError> {
    fs_link_with_follow(old_path, new_path, true, pid)
}

/// `linkat(olddirfd, oldpath, newdirfd, newpath, flags)`.
#[inline]
pub fn fs_linkat(
    olddirfd: i32,
    old_path: &[u8],
    newdirfd: i32,
    new_path: &[u8],
    flags: u32,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    const AT_SYMLINK_FOLLOW: u32 = 0x400;
    if olddirfd != AT_FDCWD && !old_path.starts_with(b"/") {
        return Err(FsBridgeError::Invalid);
    }
    if newdirfd != AT_FDCWD && !new_path.starts_with(b"/") {
        return Err(FsBridgeError::Invalid);
    }
    if flags & !AT_SYMLINK_FOLLOW != 0 {
        return Err(FsBridgeError::Invalid);
    }
    fs_link_with_follow(old_path, new_path, flags & AT_SYMLINK_FOLLOW != 0, pid)
}

/// `getdents64(fd, dirp, count)`.
#[inline]
pub fn fs_getdents64(fd: u32, dirp: u64, count: usize, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let obj_fd = resolve_fd(pid, fd)?.handle;
    if dirp == 0 {
        return Err(FsBridgeError::Fault);
    }
    if count < DIRENT64_HEADER_SIZE + 2 {
        return Err(FsBridgeError::Invalid);
    }

    let entry = OBJECT_TABLE.get(obj_fd).map_err(exofs_to_bridge_error)?;
    let data = snapshot_blob(&entry.blob_id)?;
    if !blob_is_directory(&data) {
        return Err(FsBridgeError::NotDir);
    }

    let index = PathIndex::from_bytes(&data).map_err(exofs_to_bridge_error)?;
    let entries = index.entries();
    let start_idx = entry.cursor as usize;
    if start_idx >= entries.len() {
        return Ok(0);
    }

    let mut cursor = start_idx;
    let mut written = 0usize;
    while cursor < entries.len() {
        let entry_ref = &entries[cursor];
        let name = entry_ref.name_bytes();
        let raw_size = DIRENT64_HEADER_SIZE
            .checked_add(name.len())
            .and_then(|v| v.checked_add(1))
            .ok_or(FsBridgeError::NoSpace)?;
        let reclen = (raw_size + 7) & !7usize;
        if written.saturating_add(reclen) > count {
            break;
        }

        let header = LinuxDirent64 {
            d_ino: inode_from_object_id(&entry_ref.oid),
            d_off: (cursor + 1) as i64,
            d_reclen: reclen as u16,
            d_type: dirent_type_from_kind(entry_ref.kind),
        };

        // SAFETY: LinuxDirent64 est #[repr(C)] et POD.
        let header_bytes = unsafe {
            core::slice::from_raw_parts(
                &header as *const LinuxDirent64 as *const u8,
                DIRENT64_HEADER_SIZE,
            )
        };

        let base = dirp.saturating_add(written as u64);
        copy_to_user(base as *mut u8, header_bytes.as_ptr(), header_bytes.len())
            .map_err(|_| FsBridgeError::Fault)?;
        copy_to_user(
            base.saturating_add(DIRENT64_HEADER_SIZE as u64) as *mut u8,
            name.as_ptr(),
            name.len(),
        )
        .map_err(|_| FsBridgeError::Fault)?;

        let zero = [0u8; 8];
        copy_to_user(
            base.saturating_add((DIRENT64_HEADER_SIZE + name.len()) as u64) as *mut u8,
            zero.as_ptr(),
            1,
        )
        .map_err(|_| FsBridgeError::Fault)?;

        let pad = reclen.saturating_sub(raw_size);
        if pad != 0 {
            copy_to_user(
                base.saturating_add(raw_size as u64) as *mut u8,
                zero.as_ptr(),
                pad,
            )
            .map_err(|_| FsBridgeError::Fault)?;
        }

        written = written.saturating_add(reclen);
        cursor += 1;
    }

    if written == 0 {
        return Ok(0);
    }

    OBJECT_TABLE
        .set_cursor(obj_fd, cursor as u64)
        .map_err(exofs_to_bridge_error)?;
    Ok(written as i64)
}

/// `readlink(path, buf, bufsize)`.
#[inline]
pub fn fs_readlink(path: &[u8], buf: u64, bufsize: usize, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    if buf == 0 && bufsize != 0 {
        return Err(FsBridgeError::Fault);
    }
    let normalized_path = resolve_path_with_symlinks(path, false, false)?;
    let (blob_id, kind) = path_entry(&normalized_path)?;
    if kind != PATH_INDEX_KIND_SYMLINK {
        return Err(FsBridgeError::Invalid);
    }
    let target = snapshot_blob(&blob_id)?;
    if !is_valid_symlink_target(&target) {
        return Err(FsBridgeError::Invalid);
    }
    let copy_len = target.len().min(bufsize);
    if copy_len != 0 {
        copy_to_user(buf as *mut u8, target.as_ptr(), copy_len)
            .map_err(|_| FsBridgeError::Fault)?;
    }
    Ok(copy_len as i64)
}

/// `readlinkat(dirfd, path, buf, bufsize)`.
#[inline]
pub fn fs_readlinkat(
    dirfd: i32,
    path: &[u8],
    buf: u64,
    bufsize: usize,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if dirfd == AT_FDCWD || path.starts_with(b"/") {
        return fs_readlink(path, buf, bufsize, pid);
    }
    Err(FsBridgeError::Invalid)
}

/// `truncate(path, length)`.
#[inline]
pub fn fs_truncate(path: &[u8], length: u64, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    let normalized_path = resolve_path_with_symlinks(path, true, false)?;
    let (blob_id, _) = path_entry(&normalized_path)?;
    resize_regular_blob(blob_id, length)?;
    Ok(0)
}

/// `ftruncate(fd, length)`.
#[inline]
pub fn fs_ftruncate(fd: u32, length: u64, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let obj_fd = resolve_fd(pid, fd)?.handle;
    let entry = OBJECT_TABLE.get(obj_fd).map_err(exofs_to_bridge_error)?;
    if !entry.can_write() {
        return Err(FsBridgeError::PermDenied);
    }
    resize_regular_blob(entry.blob_id, length)?;
    OBJECT_TABLE
        .set_size(obj_fd, length)
        .map_err(exofs_to_bridge_error)?;
    Ok(0)
}

/// `access(path, mode)`.
#[inline]
pub fn fs_access(path: &[u8], mode: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    if mode & !0x7 != 0 {
        return Err(FsBridgeError::Invalid);
    }
    let normalized_path = resolve_path_with_symlinks(path, true, false)?;
    let _ = path_entry(&normalized_path)?;
    Ok(0)
}

/// `fsync(fd)` / `fdatasync(fd)`.
#[inline]
pub fn fs_fsync(fd: u32, data_only: bool, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = data_only;
    let entry = OBJECT_TABLE
        .get(resolve_fd(pid, fd)?.handle)
        .map_err(exofs_to_bridge_error)?;

    if !BLOB_CACHE.contains(&entry.blob_id) {
        return Ok(0);
    }

    if let Some(data) = BLOB_CACHE.get(&entry.blob_id) {
        match object_store::persist_blob_data_if_disk(entry.blob_id, &data, true) {
            Ok(_) => {
                let _ = BLOB_CACHE.mark_clean(&entry.blob_id);
            }
            Err(err) => return Err(exofs_to_bridge_error(err)),
        }
    }

    Ok(0)
}

/// `statfs(path, buf)`.
#[inline]
pub fn fs_statfs(path: &[u8], statfs_ptr: u64, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    if statfs_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    let normalized_path = resolve_path_with_symlinks(path, true, false)?;
    let _ = path_entry(&normalized_path)?;
    write_user_typed(statfs_ptr, statfs_snapshot()).map_err(|_| FsBridgeError::Fault)?;
    Ok(0)
}

/// `fstatfs(fd, buf)`.
#[inline]
pub fn fs_fstatfs(fd: u32, statfs_ptr: u64, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if statfs_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    let _ = OBJECT_TABLE
        .get(resolve_fd(pid, fd)?.handle)
        .map_err(exofs_to_bridge_error)?;
    write_user_typed(statfs_ptr, statfs_snapshot()).map_err(|_| FsBridgeError::Fault)?;
    Ok(0)
}

/// Metadata permission calls are accepted once the target exists; ExoFS uses
/// capabilities rather than Unix ownership bits for enforcement.
#[inline]
pub fn fs_chmod(path: &[u8], mode: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    let normalized_path = resolve_path_with_symlinks(path, false, false)?;
    let (blob_id, kind) = path_entry(&normalized_path)?;
    let is_dir = kind == PATH_INDEX_KIND_DIR;
    let type_bits = default_stat_mode_for_kind(kind, is_dir) & S_IFMT;
    upsert_mode(blob_id, type_bits | (mode & 0o7777));
    Ok(0)
}

#[inline]
pub fn fs_fchmod(fd: u32, mode: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let entry = OBJECT_TABLE
        .get(resolve_fd(pid, fd)?.handle)
        .map_err(exofs_to_bridge_error)?;
    let is_dir = blob_is_directory_by_id(&entry.blob_id);
    let type_bits = if is_dir { S_IFDIR } else { S_IFREG };
    upsert_mode(entry.blob_id, type_bits | (mode & 0o7777));
    Ok(0)
}

#[inline]
pub fn fs_chown(path: &[u8], uid: u32, gid: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = (uid, gid, pid);
    let normalized_path = resolve_path_with_symlinks(path, false, false)?;
    let _ = path_entry(&normalized_path)?;
    Ok(0)
}

#[inline]
pub fn fs_fchown(fd: u32, uid: u32, gid: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = (uid, gid);
    let _ = OBJECT_TABLE
        .get(resolve_fd(pid, fd)?.handle)
        .map_err(exofs_to_bridge_error)?;
    Ok(0)
}

/// `pipe2(pipefd, flags)`.
#[inline]
pub fn fs_pipe2(fds_ptr: u64, flags: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if fds_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    if flags & !(O_CLOEXEC | O_NONBLOCK) != 0 {
        return Err(FsBridgeError::Invalid);
    }

    let blob_id = next_pseudo_blob(PSEUDO_PIPE_TAG);
    ensure_blob_exists(blob_id)?;
    let read_fd = OBJECT_TABLE
        .open(blob_id, open_flags::O_RDONLY, 0, 0, pid as u64)
        .map_err(exofs_to_bridge_error)?;
    let write_fd = match OBJECT_TABLE.open(
        blob_id,
        open_flags::O_WRONLY | open_flags::O_APPEND,
        0,
        0,
        pid as u64,
    ) {
        Ok(fd) => fd,
        Err(err) => {
            let _ = OBJECT_TABLE.close(read_fd);
            return Err(exofs_to_bridge_error(err));
        }
    };

    let mut fds = [read_fd as i32, write_fd as i32];
    let has_fd_table = process_has_fd_table(pid);
    if has_fd_table {
        let read_flags = fd_table_flags(flags, open_flags::O_RDONLY);
        let write_flags = fd_table_flags(flags, open_flags::O_WRONLY | open_flags::O_APPEND);
        let Some(read_logical) = install_process_fd(pid, read_fd as u64, read_flags) else {
            let _ = OBJECT_TABLE.close(read_fd);
            let _ = OBJECT_TABLE.close(write_fd);
            return Err(FsBridgeError::NoMemory);
        };
        let Some(write_logical) = install_process_fd(pid, write_fd as u64, write_flags) else {
            let _ = close_process_fd(pid, read_logical as u32);
            let _ = OBJECT_TABLE.close(read_fd);
            let _ = OBJECT_TABLE.close(write_fd);
            return Err(FsBridgeError::NoMemory);
        };
        fds = [read_logical, write_logical];
    }
    if write_user_typed(fds_ptr, fds).is_err() {
        if has_fd_table {
            let _ = close_process_fd(pid, fds[0] as u32);
            let _ = close_process_fd(pid, fds[1] as u32);
        }
        let _ = OBJECT_TABLE.close(read_fd);
        let _ = OBJECT_TABLE.close(write_fd);
        return Err(FsBridgeError::Fault);
    }
    Ok(0)
}

/// `eventfd2(initval, flags)`.
#[inline]
pub fn fs_eventfd2(initval: u32, flags: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if flags & !(O_CLOEXEC | O_NONBLOCK | EFD_SEMAPHORE) != 0 {
        return Err(FsBridgeError::Invalid);
    }

    let blob_id = next_pseudo_blob(PSEUDO_EVENTFD_TAG);
    store_eventfd_state(blob_id, initval as u64, flags)?;
    let fd = OBJECT_TABLE
        .open(
            blob_id,
            open_flags::O_RDWR,
            size_of::<u64>() as u64,
            0,
            pid as u64,
        )
        .map_err(exofs_to_bridge_error)?;
    if process_has_fd_table(pid) {
        if let Some(logical_fd) =
            install_process_fd(pid, fd as u64, fd_table_flags(flags, open_flags::O_RDWR))
        {
            Ok(logical_fd as i64)
        } else {
            let _ = OBJECT_TABLE.close(fd);
            Err(FsBridgeError::NoMemory)
        }
    } else {
        Ok(fd as i64)
    }
}

/// `epoll_create1(flags)`.
#[inline]
pub fn fs_epoll_create1(flags: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if flags & !EPOLL_CLOEXEC != 0 {
        return Err(FsBridgeError::Invalid);
    }
    let blob_id = next_pseudo_blob(PSEUDO_EPOLL_TAG);
    ensure_blob_exists(blob_id)?;
    let fd = OBJECT_TABLE
        .open(blob_id, open_flags::O_RDWR, 0, 0, pid as u64)
        .map_err(exofs_to_bridge_error)?;
    if process_has_fd_table(pid) {
        if let Some(logical_fd) =
            install_process_fd(pid, fd as u64, fd_table_flags(flags, open_flags::O_RDWR))
        {
            Ok(logical_fd as i64)
        } else {
            let _ = OBJECT_TABLE.close(fd);
            Err(FsBridgeError::NoMemory)
        }
    } else {
        Ok(fd as i64)
    }
}

/// `epoll_ctl(epfd, op, fd, event)`.
#[inline]
pub fn fs_epoll_ctl(
    epfd: u32,
    op: i32,
    fd: u32,
    event_ptr: u64,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let ep_obj = resolve_fd(pid, epfd)?.handle;
    let target_obj = resolve_fd(pid, fd)?.handle;
    if is_tty_handle(ep_obj) || is_tty_handle(target_obj) {
        return Err(FsBridgeError::Invalid);
    }
    let ep_entry = OBJECT_TABLE.get(ep_obj).map_err(exofs_to_bridge_error)?;
    if !is_pseudo_blob(&ep_entry.blob_id, PSEUDO_EPOLL_TAG) {
        return Err(FsBridgeError::Invalid);
    }
    let _ = OBJECT_TABLE
        .get(target_obj)
        .map_err(exofs_to_bridge_error)?;
    match op {
        EPOLL_CTL_ADD | EPOLL_CTL_MOD => {
            if event_ptr == 0 {
                return Err(FsBridgeError::Fault);
            }
            let _ =
                read_user_typed::<LinuxEpollEvent>(event_ptr).map_err(|_| FsBridgeError::Fault)?;
        }
        EPOLL_CTL_DEL => {}
        _ => return Err(FsBridgeError::Invalid),
    }
    Ok(0)
}

/// `epoll_wait(epfd, events, maxevents, timeout)`.
#[inline]
pub fn fs_epoll_wait(
    epfd: u32,
    events_ptr: u64,
    maxevents: i32,
    timeout: i32,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = timeout;
    if events_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    if maxevents <= 0 {
        return Err(FsBridgeError::Invalid);
    }
    let ep_obj = resolve_fd(pid, epfd)?.handle;
    if is_tty_handle(ep_obj) {
        return Err(FsBridgeError::Invalid);
    }
    let ep_entry = OBJECT_TABLE.get(ep_obj).map_err(exofs_to_bridge_error)?;
    if !is_pseudo_blob(&ep_entry.blob_id, PSEUDO_EPOLL_TAG) {
        return Err(FsBridgeError::Invalid);
    }
    Ok(0)
}

/// `inotify_init1(flags)`; event production is a higher layer concern.
#[inline]
pub fn fs_inotify_init1(flags: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if flags & !(O_CLOEXEC | O_NONBLOCK) != 0 {
        return Err(FsBridgeError::Invalid);
    }
    let blob_id = next_pseudo_blob(PSEUDO_INOTIFY_TAG);
    ensure_blob_exists(blob_id)?;
    let fd = OBJECT_TABLE
        .open(blob_id, open_flags::O_RDONLY, 0, 0, pid as u64)
        .map_err(exofs_to_bridge_error)?;
    if process_has_fd_table(pid) {
        if let Some(logical_fd) =
            install_process_fd(pid, fd as u64, fd_table_flags(flags, open_flags::O_RDONLY))
        {
            Ok(logical_fd as i64)
        } else {
            let _ = OBJECT_TABLE.close(fd);
            Err(FsBridgeError::NoMemory)
        }
    } else {
        Ok(fd as i64)
    }
}

/// `socketpair(AF_UNIX, SOCK_STREAM|SOCK_DGRAM, 0, sv)`.
#[inline]
pub fn fs_socketpair(
    domain: i32,
    ty: i32,
    protocol: i32,
    sv_ptr: u64,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    const AF_UNIX: i32 = 1;
    const SOCK_STREAM: i32 = 1;
    const SOCK_DGRAM: i32 = 2;
    let clean_ty = ty & !((O_NONBLOCK | O_CLOEXEC) as i32);
    if domain != AF_UNIX || protocol != 0 || (clean_ty != SOCK_STREAM && clean_ty != SOCK_DGRAM) {
        return Err(FsBridgeError::Invalid);
    }
    if sv_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }

    let a_blob = next_pseudo_blob(PSEUDO_SOCKET_TAG);
    let b_blob = next_pseudo_blob(PSEUDO_SOCKET_TAG);
    BLOB_CACHE
        .insert(a_blob, socket_blob_with_peer(b_blob))
        .map_err(exofs_to_bridge_error)?;
    BLOB_CACHE
        .insert(b_blob, socket_blob_with_peer(a_blob))
        .map_err(exofs_to_bridge_error)?;

    let a_fd = OBJECT_TABLE
        .open(a_blob, open_flags::O_RDWR, 0, 0, pid as u64)
        .map_err(exofs_to_bridge_error)?;
    let b_fd = match OBJECT_TABLE.open(b_blob, open_flags::O_RDWR, 0, 0, pid as u64) {
        Ok(fd) => fd,
        Err(err) => {
            let _ = OBJECT_TABLE.close(a_fd);
            return Err(exofs_to_bridge_error(err));
        }
    };
    let mut sv = [a_fd as i32, b_fd as i32];
    let has_fd_table = process_has_fd_table(pid);
    if has_fd_table {
        let fd_flags = fd_table_flags(ty as u32, open_flags::O_RDWR);
        let Some(a_logical) = install_process_fd(pid, a_fd as u64, fd_flags) else {
            let _ = OBJECT_TABLE.close(a_fd);
            let _ = OBJECT_TABLE.close(b_fd);
            return Err(FsBridgeError::NoMemory);
        };
        let Some(b_logical) = install_process_fd(pid, b_fd as u64, fd_flags) else {
            let _ = close_process_fd(pid, a_logical as u32);
            let _ = OBJECT_TABLE.close(a_fd);
            let _ = OBJECT_TABLE.close(b_fd);
            return Err(FsBridgeError::NoMemory);
        };
        sv = [a_logical, b_logical];
    }
    if write_user_typed(sv_ptr, sv).is_err() {
        if has_fd_table {
            let _ = close_process_fd(pid, sv[0] as u32);
            let _ = close_process_fd(pid, sv[1] as u32);
        }
        let _ = OBJECT_TABLE.close(a_fd);
        let _ = OBJECT_TABLE.close(b_fd);
        return Err(FsBridgeError::Fault);
    }
    Ok(0)
}

/// `mknod(path, mode, dev)` for regular files/FIFOs in the ExoFS namespace.
#[inline]
pub fn fs_mknod(path: &[u8], mode: u32, dev: u64, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = (dev, pid);
    let kind = mode & S_IFMT;
    if kind != 0 && kind != S_IFREG && kind != S_IFIFO {
        return Err(FsBridgeError::Invalid);
    }
    let fd = fs_open(
        path,
        open_flags::O_CREAT | open_flags::O_EXCL | open_flags::O_RDWR,
        mode,
        pid,
    )? as u32;
    let _ = fs_close(fd, pid);
    Ok(0)
}

/// `mknodat(dirfd, path, mode, dev)`.
#[inline]
pub fn fs_mknodat(
    dirfd: i32,
    path: &[u8],
    mode: u32,
    dev: u64,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if dirfd == AT_FDCWD || path.starts_with(b"/") {
        return fs_mknod(path, mode, dev, pid);
    }
    Err(FsBridgeError::Invalid)
}

/// `splice(fd_in, off_in, fd_out, off_out, len, flags)`.
#[inline]
pub fn fs_splice(
    fd_in: u32,
    off_in_ptr: u64,
    fd_out: u32,
    off_out_ptr: u64,
    len: usize,
    flags: u32,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if flags & !0x0F != 0 {
        return Err(FsBridgeError::Invalid);
    }
    let in_fd = resolve_fd(pid, fd_in)?.handle;
    let out_fd = resolve_fd(pid, fd_out)?.handle;
    let in_entry = OBJECT_TABLE.get(in_fd).map_err(exofs_to_bridge_error)?;
    let out_entry = OBJECT_TABLE.get(out_fd).map_err(exofs_to_bridge_error)?;
    if !in_entry.can_read() || !out_entry.can_write() {
        return Err(FsBridgeError::PermDenied);
    }

    let input_is_pipe = is_pseudo_blob(&in_entry.blob_id, PSEUDO_PIPE_TAG);
    let output_is_pipe = is_pseudo_blob(&out_entry.blob_id, PSEUDO_PIPE_TAG);
    let input_is_socket = is_pseudo_blob(&in_entry.blob_id, PSEUDO_SOCKET_TAG);
    let output_is_socket = is_pseudo_blob(&out_entry.blob_id, PSEUDO_SOCKET_TAG);

    let data = if input_is_pipe {
        read_pipe_payload(in_entry.blob_id, len, true)?
    } else if input_is_socket {
        read_socket_payload(in_entry.blob_id, len, true)?
    } else {
        let offset = if off_in_ptr == 0 {
            in_entry.cursor
        } else {
            read_signed_offset(off_in_ptr)?
        };
        let data = read_blob_bytes_at(in_entry.blob_id, offset, len)?;
        if off_in_ptr == 0 {
            OBJECT_TABLE
                .set_cursor(in_fd, offset.saturating_add(data.len() as u64))
                .map_err(exofs_to_bridge_error)?;
        } else {
            write_user_typed(
                off_in_ptr,
                (offset.saturating_add(data.len() as u64)) as i64,
            )
            .map_err(|_| FsBridgeError::Fault)?;
        }
        data
    };
    if data.is_empty() {
        return Ok(0);
    }

    if output_is_pipe {
        append_pipe_payload(out_entry.blob_id, &data)
    } else if output_is_socket {
        let peer = socket_peer_blob(out_entry.blob_id)?;
        append_socket_payload(peer, &data)
    } else {
        let offset = if off_out_ptr == 0 {
            out_entry.cursor
        } else {
            read_signed_offset(off_out_ptr)?
        };
        let written = write_blob_bytes_at(out_entry.blob_id, offset, &data)? as u64;
        if off_out_ptr == 0 {
            OBJECT_TABLE
                .set_cursor(out_fd, offset.saturating_add(written))
                .map_err(exofs_to_bridge_error)?;
        } else {
            write_user_typed(off_out_ptr, (offset.saturating_add(written)) as i64)
                .map_err(|_| FsBridgeError::Fault)?;
        }
        OBJECT_TABLE
            .set_size(out_fd, blob_len(&out_entry.blob_id) as u64)
            .map_err(exofs_to_bridge_error)?;
        Ok(written as i64)
    }
}

/// `tee(fd_in, fd_out, len, flags)` duplicates pipe data without consuming it.
#[inline]
pub fn fs_tee(
    fd_in: u32,
    fd_out: u32,
    len: usize,
    flags: u32,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if flags & !0x0F != 0 {
        return Err(FsBridgeError::Invalid);
    }
    let in_entry = OBJECT_TABLE
        .get(resolve_fd(pid, fd_in)?.handle)
        .map_err(exofs_to_bridge_error)?;
    let out_entry = OBJECT_TABLE
        .get(resolve_fd(pid, fd_out)?.handle)
        .map_err(exofs_to_bridge_error)?;
    if !is_pseudo_blob(&in_entry.blob_id, PSEUDO_PIPE_TAG)
        || !is_pseudo_blob(&out_entry.blob_id, PSEUDO_PIPE_TAG)
    {
        return Err(FsBridgeError::Invalid);
    }
    let data = read_pipe_payload(in_entry.blob_id, len, false)?;
    append_pipe_payload(out_entry.blob_id, &data)
}

/// `vmsplice(fd, iov, nr_segs, flags)` writes user iovecs into a pipe.
#[inline]
pub fn fs_vmsplice(
    fd: u32,
    iov_ptr: u64,
    iovcnt: u32,
    flags: u32,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if flags & !0x0F != 0 {
        return Err(FsBridgeError::Invalid);
    }
    let entry = OBJECT_TABLE
        .get(resolve_fd(pid, fd)?.handle)
        .map_err(exofs_to_bridge_error)?;
    if !entry.can_write() || !is_pseudo_blob(&entry.blob_id, PSEUDO_PIPE_TAG) {
        return Err(FsBridgeError::Invalid);
    }
    vectored_io(iov_ptr, iovcnt, |base, len| {
        let input = read_user_bytes(base, len)?;
        append_pipe_payload(entry.blob_id, &input)
    })
}

/// `poll(fds, nfds, timeout)`.
#[inline]
pub fn fs_poll(fds_ptr: u64, nfds: usize, timeout: i32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = (timeout, pid);
    if nfds > 1024 {
        return Err(FsBridgeError::Invalid);
    }
    if nfds == 0 {
        return Ok(0);
    }
    if fds_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }

    let mut ready = 0i64;
    let mut i = 0usize;
    while i < nfds {
        let addr = fds_ptr
            .checked_add((i as u64).saturating_mul(size_of::<LinuxPollFd>() as u64))
            .ok_or(FsBridgeError::Fault)?;
        let mut pfd = read_user_typed::<LinuxPollFd>(addr).map_err(|_| FsBridgeError::Fault)?;
        pfd.revents = 0;
        if pfd.fd >= 0 {
            match fd_readiness(pfd.fd as u32, pid) {
                Ok((readable, writable)) => {
                    if readable && (pfd.events & POLLIN) != 0 {
                        pfd.revents |= POLLIN;
                    }
                    if writable && (pfd.events & POLLOUT) != 0 {
                        pfd.revents |= POLLOUT;
                    }
                }
                Err(_) => {
                    pfd.revents |= POLLNVAL;
                }
            }
        }
        if pfd.revents != 0 {
            ready += 1;
        }
        write_user_typed(addr, pfd).map_err(|_| FsBridgeError::Fault)?;
        i += 1;
    }
    Ok(ready)
}

/// `select(nfds, readfds, writefds, exceptfds, timeout)`.
#[inline]
pub fn fs_select(
    nfds: usize,
    readfds_ptr: u64,
    writefds_ptr: u64,
    exceptfds_ptr: u64,
    timeout_ptr: u64,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = (timeout_ptr, pid);
    if nfds > 1024 {
        return Err(FsBridgeError::Invalid);
    }
    let bytes = nfds.saturating_add(7) / 8;
    let mut readfds = vec_with_zeroes(bytes)?;
    let mut writefds = vec_with_zeroes(bytes)?;
    let mut exceptfds = vec_with_zeroes(bytes)?;

    if readfds_ptr != 0 && bytes != 0 {
        copy_from_user(readfds.as_mut_ptr(), readfds_ptr as *const u8, bytes)
            .map_err(|_| FsBridgeError::Fault)?;
    }
    if writefds_ptr != 0 && bytes != 0 {
        copy_from_user(writefds.as_mut_ptr(), writefds_ptr as *const u8, bytes)
            .map_err(|_| FsBridgeError::Fault)?;
    }
    if exceptfds_ptr != 0 && bytes != 0 {
        copy_from_user(exceptfds.as_mut_ptr(), exceptfds_ptr as *const u8, bytes)
            .map_err(|_| FsBridgeError::Fault)?;
    }

    let mut ready = 0i64;
    let mut fd = 0usize;
    while fd < nfds {
        if readfds_ptr != 0 && fdset_bit(&readfds, fd) {
            let keep = fd_readiness(fd as u32, pid)
                .map(|(readable, _)| readable)
                .unwrap_or(false);
            set_fdset_bit(&mut readfds, fd, keep);
            if keep {
                ready += 1;
            }
        }
        if writefds_ptr != 0 && fdset_bit(&writefds, fd) {
            let keep = fd_readiness(fd as u32, pid)
                .map(|(_, writable)| writable)
                .unwrap_or(false);
            set_fdset_bit(&mut writefds, fd, keep);
            if keep {
                ready += 1;
            }
        }
        if exceptfds_ptr != 0 && fdset_bit(&exceptfds, fd) {
            set_fdset_bit(&mut exceptfds, fd, false);
        }
        fd += 1;
    }

    if readfds_ptr != 0 && bytes != 0 {
        copy_to_user(readfds_ptr as *mut u8, readfds.as_ptr(), bytes)
            .map_err(|_| FsBridgeError::Fault)?;
    }
    if writefds_ptr != 0 && bytes != 0 {
        copy_to_user(writefds_ptr as *mut u8, writefds.as_ptr(), bytes)
            .map_err(|_| FsBridgeError::Fault)?;
    }
    if exceptfds_ptr != 0 && bytes != 0 {
        copy_to_user(exceptfds_ptr as *mut u8, exceptfds.as_ptr(), bytes)
            .map_err(|_| FsBridgeError::Fault)?;
    }
    Ok(ready)
}

#[inline]
fn vec_with_zeroes(len: usize) -> Result<Vec<u8>, FsBridgeError> {
    let mut out = Vec::new();
    out.try_reserve_exact(len)
        .map_err(|_| FsBridgeError::NoMemory)?;
    out.resize(len, 0);
    Ok(out)
}

#[inline]
fn read_user_bytes(ptr: u64, len: usize) -> Result<Vec<u8>, FsBridgeError> {
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

/// `copy_file_range`.
#[inline]
pub fn fs_copy_file_range(
    fd_in: u32,
    off_in_ptr: u64,
    fd_out: u32,
    off_out_ptr: u64,
    len: usize,
    flags: u32,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if flags != 0 {
        return Err(FsBridgeError::Invalid);
    }
    let in_fd = resolve_fd(pid, fd_in)?.handle;
    let out_fd = resolve_fd(pid, fd_out)?.handle;
    let in_entry = OBJECT_TABLE.get(in_fd).map_err(exofs_to_bridge_error)?;
    let out_entry = OBJECT_TABLE.get(out_fd).map_err(exofs_to_bridge_error)?;
    if !in_entry.can_read() || !out_entry.can_write() {
        return Err(FsBridgeError::PermDenied);
    }

    let in_offset = if off_in_ptr == 0 {
        in_entry.cursor
    } else {
        read_signed_offset(off_in_ptr)?
    };
    let out_offset = if off_out_ptr == 0 {
        out_entry.cursor
    } else {
        read_signed_offset(off_out_ptr)?
    };

    let data = read_blob_bytes_at(in_entry.blob_id, in_offset, len)?;
    if data.is_empty() {
        return Ok(0);
    }
    let written = write_blob_bytes_at(out_entry.blob_id, out_offset, &data)? as u64;
    if off_in_ptr == 0 {
        OBJECT_TABLE
            .set_cursor(in_fd, in_offset.saturating_add(written))
            .map_err(exofs_to_bridge_error)?;
    } else {
        write_user_typed(off_in_ptr, (in_offset.saturating_add(written)) as i64)
            .map_err(|_| FsBridgeError::Fault)?;
    }
    if off_out_ptr == 0 {
        OBJECT_TABLE
            .set_cursor(out_fd, out_offset.saturating_add(written))
            .map_err(exofs_to_bridge_error)?;
    } else {
        write_user_typed(off_out_ptr, (out_offset.saturating_add(written)) as i64)
            .map_err(|_| FsBridgeError::Fault)?;
    }
    OBJECT_TABLE
        .set_size(out_fd, blob_len(&out_entry.blob_id) as u64)
        .map_err(exofs_to_bridge_error)?;
    Ok(written as i64)
}

/// `sendfile(out_fd, in_fd, offset, count)`.
#[inline]
pub fn fs_sendfile(
    out_fd: u32,
    in_fd: u32,
    offset_ptr: u64,
    count: usize,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let in_fd = resolve_fd(pid, in_fd)?.handle;
    let out_fd = resolve_fd(pid, out_fd)?.handle;
    let in_entry = OBJECT_TABLE.get(in_fd).map_err(exofs_to_bridge_error)?;
    let out_entry = OBJECT_TABLE.get(out_fd).map_err(exofs_to_bridge_error)?;
    if !in_entry.can_read() || !out_entry.can_write() {
        return Err(FsBridgeError::PermDenied);
    }
    let in_offset = if offset_ptr == 0 {
        in_entry.cursor
    } else {
        read_signed_offset(offset_ptr)?
    };
    let out_offset = out_entry.cursor;
    let data = read_blob_bytes_at(in_entry.blob_id, in_offset, count)?;
    if data.is_empty() {
        return Ok(0);
    }
    let written = write_blob_bytes_at(out_entry.blob_id, out_offset, &data)? as u64;
    if offset_ptr == 0 {
        OBJECT_TABLE
            .set_cursor(in_fd, in_offset.saturating_add(written))
            .map_err(exofs_to_bridge_error)?;
    } else {
        write_user_typed(offset_ptr, (in_offset.saturating_add(written)) as i64)
            .map_err(|_| FsBridgeError::Fault)?;
    }
    OBJECT_TABLE
        .set_cursor(out_fd, out_offset.saturating_add(written))
        .map_err(exofs_to_bridge_error)?;
    OBJECT_TABLE
        .set_size(out_fd, blob_len(&out_entry.blob_id) as u64)
        .map_err(exofs_to_bridge_error)?;
    Ok(written as i64)
}

#[inline]
fn read_signed_offset(ptr: u64) -> Result<u64, FsBridgeError> {
    let value = read_user_typed::<i64>(ptr).map_err(|_| FsBridgeError::Fault)?;
    if value < 0 {
        return Err(FsBridgeError::Invalid);
    }
    Ok(value as u64)
}

/// `fallocate(fd, mode, offset, len)`.
#[inline]
pub fn fs_fallocate(
    fd: u32,
    mode: u32,
    offset: u64,
    len: u64,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if len == 0 || mode & !FALLOC_FL_KEEP_SIZE != 0 {
        return Err(FsBridgeError::Invalid);
    }
    let obj_fd = resolve_fd(pid, fd)?.handle;
    let entry = OBJECT_TABLE.get(obj_fd).map_err(exofs_to_bridge_error)?;
    if !entry.can_write() {
        return Err(FsBridgeError::PermDenied);
    }
    let end = offset.checked_add(len).ok_or(FsBridgeError::Invalid)?;
    if mode & FALLOC_FL_KEEP_SIZE == 0 && end > blob_len(&entry.blob_id) as u64 {
        resize_regular_blob(entry.blob_id, end)?;
        OBJECT_TABLE
            .set_size(obj_fd, end)
            .map_err(exofs_to_bridge_error)?;
    } else {
        ensure_blob_exists(entry.blob_id)?;
    }
    Ok(0)
}

/// `sync_file_range(fd, offset, nbytes, flags)`.
#[inline]
pub fn fs_sync_file_range(
    fd: u32,
    offset: u64,
    nbytes: u64,
    flags: u32,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = (offset, nbytes, pid);
    let allowed = SYNC_FILE_RANGE_WAIT_BEFORE | SYNC_FILE_RANGE_WRITE | SYNC_FILE_RANGE_WAIT_AFTER;
    if flags & !allowed != 0 {
        return Err(FsBridgeError::Invalid);
    }
    fs_fsync(fd, false, pid)
}

/// `sync()`.
#[inline]
pub fn fs_sync(pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;

    let dirty = BLOB_CACHE.collect_dirty();
    if dirty.is_empty() {
        return Ok(0);
    }

    let mut io_err = false;
    let mut i = 0usize;
    while i < dirty.len() {
        let (blob_id, ref data) = dirty[i];
        match object_store::persist_blob_data_if_disk(blob_id, data, true) {
            Ok(_) => {
                let _ = BLOB_CACHE.mark_clean(&blob_id);
            }
            Err(_) => {
                io_err = true;
            }
        }
        i = i.wrapping_add(1);
    }

    if io_err {
        return Err(FsBridgeError::Io);
    }

    Ok(0)
}

/// `posix_fadvise/fadvise64`.
#[inline]
pub fn fs_fadvise64(
    fd: u32,
    offset: u64,
    len: u64,
    advice: u32,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = (offset, len);
    if advice > 5 {
        return Err(FsBridgeError::Invalid);
    }
    let _ = OBJECT_TABLE
        .get(resolve_fd(pid, fd)?.handle)
        .map_err(exofs_to_bridge_error)?;
    Ok(0)
}

/// `ioctl(fd, request, arg)`.
#[inline]
pub fn fs_ioctl(fd: u32, request: u64, arg: u64, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let entry = OBJECT_TABLE
        .get(resolve_fd(pid, fd)?.handle)
        .map_err(exofs_to_bridge_error)?;
    match request {
        FIONREAD => {
            if arg == 0 {
                return Err(FsBridgeError::Fault);
            }
            let available = if is_pseudo_blob(&entry.blob_id, PSEUDO_EVENTFD_TAG) {
                eventfd_state(entry.blob_id)
                    .map(|(value, _)| if value == 0 { 0 } else { 8 })
                    .unwrap_or(0)
            } else if is_pseudo_blob(&entry.blob_id, PSEUDO_PIPE_TAG) {
                blob_len(&entry.blob_id)
            } else if is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG) {
                socket_payload_len(entry.blob_id)
            } else {
                blob_len(&entry.blob_id).saturating_sub(entry.cursor as usize)
            };
            write_user_typed(arg, available as i32).map_err(|_| FsBridgeError::Fault)?;
            Ok(0)
        }
        _ => Err(FsBridgeError::Invalid),
    }
}

/// `statx(dirfd, path, flags, mask, statxbuf)`.
#[inline]
pub fn fs_statx(
    dirfd: i32,
    path: &[u8],
    flags: u32,
    mask: u32,
    statx_ptr: u64,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    const AT_SYMLINK_NOFOLLOW: u32 = 0x100;
    const AT_EMPTY_PATH: u32 = 0x1000;
    let _ = mask;
    if statx_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    if flags & !(AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH) != 0 {
        return Err(FsBridgeError::Invalid);
    }
    if dirfd != AT_FDCWD && !path.starts_with(b"/") {
        return Err(FsBridgeError::Invalid);
    }
    let follow = flags & AT_SYMLINK_NOFOLLOW == 0;
    let normalized_path = resolve_path_with_symlinks(path, follow, false)?;
    let (blob_id, kind) = path_entry(&normalized_path)?;
    let statx = linux_statx_for_blob_meta(
        blob_id,
        blob_len(&blob_id) as u64,
        pid,
        kind,
        kind == PATH_INDEX_KIND_DIR,
    );
    write_user_typed(statx_ptr, statx).map_err(|_| FsBridgeError::Fault)?;
    Ok(0)
}

/// `getcwd(buf, size)`; ExoFS currently exposes a process-neutral root cwd.
#[inline]
pub fn fs_getcwd(buf_ptr: u64, size: usize, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    if buf_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    if size < 2 {
        return Err(FsBridgeError::Invalid);
    }
    let cwd = [b'/', 0];
    copy_to_user(buf_ptr as *mut u8, cwd.as_ptr(), cwd.len()).map_err(|_| FsBridgeError::Fault)?;
    Ok(cwd.len() as i64)
}

/// `chdir(path)`.
#[inline]
pub fn fs_chdir(path: &[u8], pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    let normalized_path = resolve_path_with_symlinks(path, true, false)?;
    let (_, kind) = path_entry(&normalized_path)?;
    if kind != PATH_INDEX_KIND_DIR {
        return Err(FsBridgeError::NotDir);
    }
    Ok(0)
}

/// `fchdir(fd)`.
#[inline]
pub fn fs_fchdir(fd: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let entry = OBJECT_TABLE
        .get(resolve_fd(pid, fd)?.handle)
        .map_err(exofs_to_bridge_error)?;
    if !blob_is_directory_by_id(&entry.blob_id) {
        return Err(FsBridgeError::NotDir);
    }
    Ok(0)
}

/// `umask(mask)`.
#[inline]
pub fn fs_umask(mask: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let old = swap_process_umask(pid, mask);
    Ok(old as i64)
}

/// `getrlimit(resource, rlim)`.
#[inline]
pub fn fs_getrlimit(resource: u32, rlim_ptr: u64, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    if resource >= RLIMIT_NLIMITS {
        return Err(FsBridgeError::Invalid);
    }
    if rlim_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    let limit = if resource == RLIMIT_NOFILE {
        LinuxRlimit {
            rlim_cur: 65_536,
            rlim_max: 65_536,
        }
    } else {
        LinuxRlimit {
            rlim_cur: u64::MAX,
            rlim_max: u64::MAX,
        }
    };
    write_user_typed(rlim_ptr, limit).map_err(|_| FsBridgeError::Fault)?;
    Ok(0)
}

/// `setrlimit(resource, rlim)`.
#[inline]
pub fn fs_setrlimit(resource: u32, rlim_ptr: u64, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    if resource >= RLIMIT_NLIMITS {
        return Err(FsBridgeError::Invalid);
    }
    if rlim_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    let limit = read_user_typed::<LinuxRlimit>(rlim_ptr).map_err(|_| FsBridgeError::Fault)?;
    if limit.rlim_cur > limit.rlim_max {
        return Err(FsBridgeError::Invalid);
    }
    Ok(0)
}

// ─────────────────────────────────────────────────────────────────────────────
// Helper : conversion automatique pour les syscall handlers
// ─────────────────────────────────────────────────────────────────────────────

/// Convertit un `Result<i64, FsBridgeError>` en code de retour syscall.
#[inline(always)]
pub fn bridge_result(r: Result<i64, FsBridgeError>) -> i64 {
    match r {
        Ok(n) => n,
        Err(e) => e.to_errno(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_bridge() {
        unsafe {
            fs_bridge_init();
        }
        OBJECT_TABLE.reset_all();
        BLOB_CACHE.flush_all_force();
    }

    #[test]
    fn test_fs_bridge_open_write_read_lseek_roundtrip() {
        init_bridge();

        let path = b"/etc/exo/kernel.toml";
        let payload = *b"exo-kernel-config";
        let mut read_back = [0u8; 17];

        let fd = fs_open(path, open_flags::O_RDWR | open_flags::O_CREAT, 0, 7).unwrap() as u32;
        assert_eq!(
            fs_write(fd, payload.as_ptr() as u64, payload.len(), 7).unwrap(),
            payload.len() as i64
        );
        assert_eq!(fs_lseek(fd, 0, SEEK_SET, 7).unwrap(), 0);
        assert_eq!(
            fs_read(fd, read_back.as_mut_ptr() as u64, read_back.len(), 7).unwrap(),
            read_back.len() as i64
        );
        assert_eq!(&read_back, &payload);
        assert_eq!(fs_close(fd, 7).unwrap(), 0);
    }

    #[test]
    fn test_fs_bridge_roundtrip_stress() {
        init_bridge();

        for idx in 0..512u32 {
            let mut path = [0u8; 32];
            let prefix_len = 8;
            path[..prefix_len].copy_from_slice(b"/stress/");
            let suffix_len = write_u32_hex(&mut path[prefix_len..], idx);

            let write_len = (idx as usize % 48) + 1;
            let mut payload = [0u8; 64];
            for (off, byte) in payload[..write_len].iter_mut().enumerate() {
                *byte = idx.wrapping_add(off as u32) as u8;
            }

            let fd = fs_open(
                &path[..prefix_len + suffix_len],
                open_flags::O_RDWR | open_flags::O_CREAT | open_flags::O_TRUNC,
                0,
                11,
            )
            .unwrap() as u32;
            assert_eq!(
                fs_write(fd, payload.as_ptr() as u64, write_len, 11).unwrap(),
                write_len as i64
            );
            assert_eq!(fs_lseek(fd, 0, SEEK_SET, 11).unwrap(), 0);

            let mut out = [0u8; 64];
            assert_eq!(
                fs_read(fd, out.as_mut_ptr() as u64, write_len, 11).unwrap(),
                write_len as i64
            );
            assert_eq!(&out[..write_len], &payload[..write_len]);
            assert_eq!(fs_close(fd, 11).unwrap(), 0);
        }
    }

    #[test]
    fn test_fs_openat_at_fdcwd_aliases_open() {
        init_bridge();

        let path = b"relative/exo.conf";
        let fd = fs_openat(
            AT_FDCWD,
            path,
            open_flags::O_RDWR | open_flags::O_CREAT,
            0,
            17,
        )
        .unwrap() as u32;
        assert!(fd >= crate::fs::exofs::syscall::object_fd::FD_RESERVED);
        assert_eq!(fs_close(fd, 17).unwrap(), 0);
    }

    #[test]
    fn test_fs_openat_relative_dirfd_is_rejected() {
        init_bridge();

        let err = fs_openat(9, b"relative/exo.conf", open_flags::O_RDONLY, 0, 17).unwrap_err();
        assert_eq!(err, FsBridgeError::Invalid);
    }

    #[test]
    fn test_fs_open_accepts_linux_descriptor_flags_as_noops() {
        init_bridge();

        let fd = fs_open(
            b"/flags/nonblock.txt",
            open_flags::O_CREAT | open_flags::O_RDWR | O_NONBLOCK | O_CLOEXEC,
            0,
            18,
        )
        .unwrap() as u32;
        assert_eq!(fs_close(fd, 18).unwrap(), 0);
    }

    #[test]
    fn test_fs_stat_and_fstat_report_size_and_kind() {
        init_bridge();

        let dir_path = b"/var/exo";
        assert_eq!(fs_mkdir(dir_path, 0, 21).unwrap(), 0);

        let mut dir_stat = LinuxStat::default();
        assert_eq!(
            fs_stat(dir_path, &mut dir_stat as *mut _ as u64, 21).unwrap(),
            0
        );
        assert_eq!(
            dir_stat.st_mode & STAT_MODE_MASK,
            STAT_MODE_DIR & STAT_MODE_MASK
        );

        let file_path = b"/var/exo/runtime.cfg";
        let payload = *b"exo-runtime";
        let fd =
            fs_open(file_path, open_flags::O_RDWR | open_flags::O_CREAT, 0, 21).unwrap() as u32;
        assert_eq!(
            fs_write(fd, payload.as_ptr() as u64, payload.len(), 21).unwrap(),
            payload.len() as i64
        );

        let mut path_stat = LinuxStat::default();
        let mut fd_stat = LinuxStat::default();
        assert_eq!(
            fs_stat(file_path, &mut path_stat as *mut _ as u64, 21).unwrap(),
            0
        );
        assert_eq!(fs_fstat(fd, &mut fd_stat as *mut _ as u64, 21).unwrap(), 0);
        assert_eq!(path_stat.st_size, payload.len() as i64);
        assert_eq!(fd_stat.st_size, payload.len() as i64);
        assert_eq!(
            fd_stat.st_mode & STAT_MODE_MASK,
            STAT_MODE_FILE & STAT_MODE_MASK
        );
        assert_eq!(fs_close(fd, 21).unwrap(), 0);
    }

    #[test]
    fn test_fs_mkdir_rmdir_unlink_roundtrip() {
        init_bridge();

        let dir_path = b"/srv/exo";
        assert_eq!(fs_mkdir(dir_path, 0, 29).unwrap(), 0);
        assert_eq!(fs_unlink(dir_path, 29).unwrap_err(), FsBridgeError::IsDir);
        assert_eq!(fs_rmdir(dir_path, 29).unwrap(), 0);
        let mut dir_stat = LinuxStat::default();
        assert_eq!(
            fs_stat(dir_path, &mut dir_stat as *mut _ as u64, 29).unwrap_err(),
            FsBridgeError::NotFound
        );

        let file_path = b"/srv/exo.log";
        let fd =
            fs_open(file_path, open_flags::O_CREAT | open_flags::O_RDWR, 0, 29).unwrap() as u32;
        assert_eq!(fs_close(fd, 29).unwrap(), 0);
        assert_eq!(fs_unlink(file_path, 29).unwrap(), 0);
        let mut file_stat = LinuxStat::default();
        assert_eq!(
            fs_stat(file_path, &mut file_stat as *mut _ as u64, 29).unwrap_err(),
            FsBridgeError::NotFound
        );
    }

    #[test]
    fn test_fs_directory_lifecycle_stress() {
        init_bridge();

        for idx in 0..256u32 {
            let mut dir_path = [0u8; 40];
            let dir_prefix = b"/dirs/";
            dir_path[..dir_prefix.len()].copy_from_slice(dir_prefix);
            let dir_len = dir_prefix.len() + write_u32_hex(&mut dir_path[dir_prefix.len()..], idx);

            assert_eq!(fs_mkdir(&dir_path[..dir_len], 0, 42).unwrap(), 0);
            let mut dir_stat = LinuxStat::default();
            assert_eq!(
                fs_stat(&dir_path[..dir_len], &mut dir_stat as *mut _ as u64, 42).unwrap(),
                0
            );
            assert_eq!(
                dir_stat.st_mode & STAT_MODE_MASK,
                STAT_MODE_DIR & STAT_MODE_MASK
            );
            assert_eq!(fs_rmdir(&dir_path[..dir_len], 42).unwrap(), 0);

            let mut file_path = [0u8; 48];
            let file_prefix = b"/files/";
            file_path[..file_prefix.len()].copy_from_slice(file_prefix);
            let file_len =
                file_prefix.len() + write_u32_hex(&mut file_path[file_prefix.len()..], idx);
            let fd = fs_open(
                &file_path[..file_len],
                open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
                0,
                42,
            )
            .unwrap() as u32;
            assert_eq!(fs_close(fd, 42).unwrap(), 0);
            assert_eq!(fs_unlink(&file_path[..file_len], 42).unwrap(), 0);
        }
    }

    fn parse_dirent_names(buf: &[u8]) -> Vec<Vec<u8>> {
        let mut names = Vec::new();
        let mut off = 0usize;
        while off + DIRENT64_HEADER_SIZE <= buf.len() {
            let reclen = u16::from_le_bytes([buf[off + 16], buf[off + 17]]) as usize;
            if reclen == 0 || off + reclen > buf.len() {
                break;
            }
            let name_start = off + DIRENT64_HEADER_SIZE;
            let name_end = off + reclen;
            let mut cursor = name_start;
            while cursor < name_end && buf[cursor] != 0 {
                cursor += 1;
            }
            names.push(buf[name_start..cursor].to_vec());
            off += reclen;
        }
        names
    }

    #[test]
    fn test_fs_getdents64_reports_created_children() {
        init_bridge();

        assert_eq!(fs_mkdir(b"/etc", 0, 51).unwrap(), 0);
        let file_fd = fs_open(
            b"/etc/hosts",
            open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
            0,
            51,
        )
        .unwrap() as u32;
        assert_eq!(fs_close(file_fd, 51).unwrap(), 0);

        let dir_fd = fs_open(b"/etc", open_flags::O_RDONLY, 0, 51).unwrap() as u32;
        let mut out = [0u8; 256];
        let n = fs_getdents64(dir_fd, out.as_mut_ptr() as u64, out.len(), 51).unwrap() as usize;
        let names = parse_dirent_names(&out[..n]);
        assert!(names.iter().any(|name| name.as_slice() == b"hosts"));
        assert_eq!(fs_close(dir_fd, 51).unwrap(), 0);
    }

    #[test]
    fn test_fs_shell_minimal_command_graph() {
        init_bridge();

        assert_eq!(fs_mkdir(b"/auto/chain", 0, 91).unwrap(), 0);
        assert_eq!(fs_rmdir(b"/auto/chain", 91).unwrap(), 0);
        assert_eq!(fs_rmdir(b"/auto", 91).unwrap(), 0);

        assert_eq!(fs_mkdir(b"/tmp", 0, 91).unwrap(), 0);
        assert_eq!(fs_mkdir(b"/tmp/t", 0, 91).unwrap(), 0);

        let fd = fs_open(
            b"/tmp/t/a",
            open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
            0,
            91,
        )
        .unwrap() as u32;
        let payload = *b"hello-terminal";
        assert_eq!(
            fs_write(fd, payload.as_ptr() as u64, payload.len(), 91).unwrap(),
            payload.len() as i64
        );
        assert_eq!(fs_lseek(fd, 0, SEEK_SET, 91).unwrap(), 0);
        let mut out = [0u8; 32];
        let n = fs_read(fd, out.as_mut_ptr() as u64, out.len(), 91).unwrap() as usize;
        assert_eq!(&out[..n], &payload);
        assert_eq!(fs_close(fd, 91).unwrap(), 0);

        let dir_fd = fs_open(b"/tmp/t", open_flags::O_RDONLY, 0, 91).unwrap() as u32;
        let mut dirents = [0u8; 256];
        let n =
            fs_getdents64(dir_fd, dirents.as_mut_ptr() as u64, dirents.len(), 91).unwrap() as usize;
        let names = parse_dirent_names(&dirents[..n]);
        assert!(names.iter().any(|name| name.as_slice() == b"a"));
        assert_eq!(fs_close(dir_fd, 91).unwrap(), 0);

        let copy_fd = fs_open(
            b"/tmp/t/copy",
            open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
            0,
            91,
        )
        .unwrap() as u32;
        assert_eq!(
            fs_write(copy_fd, payload.as_ptr() as u64, payload.len(), 91).unwrap(),
            payload.len() as i64
        );
        assert_eq!(fs_close(copy_fd, 91).unwrap(), 0);
        assert_eq!(fs_rename(b"/tmp/t/copy", b"/tmp/t/moved", 91).unwrap(), 0);

        let moved_fd = fs_open(b"/tmp/t/moved", open_flags::O_RDONLY, 0, 91).unwrap() as u32;
        let mut moved = [0u8; 32];
        let n = fs_read(moved_fd, moved.as_mut_ptr() as u64, moved.len(), 91).unwrap() as usize;
        assert_eq!(&moved[..n], &payload);
        assert_eq!(fs_close(moved_fd, 91).unwrap(), 0);

        assert_eq!(fs_unlink(b"/tmp/t/a", 91).unwrap(), 0);
        assert_eq!(fs_unlink(b"/tmp/t/moved", 91).unwrap(), 0);
        assert_eq!(fs_rmdir(b"/tmp/t", 91).unwrap(), 0);
        assert_eq!(fs_rmdir(b"/tmp", 91).unwrap(), 0);
    }

    #[test]
    fn test_fs_getdents64_stress_tracks_many_children() {
        init_bridge();

        assert_eq!(fs_mkdir(b"/stressdir", 0, 52).unwrap(), 0);
        for idx in 0..64u32 {
            let mut path = [0u8; 32];
            let prefix = b"/stressdir/f";
            path[..prefix.len()].copy_from_slice(prefix);
            let len = prefix.len() + write_u32_hex(&mut path[prefix.len()..], idx);
            let fd = fs_open(
                &path[..len],
                open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
                0,
                52,
            )
            .unwrap() as u32;
            assert_eq!(fs_close(fd, 52).unwrap(), 0);
        }

        let dir_fd = fs_open(b"/stressdir", open_flags::O_RDONLY, 0, 52).unwrap() as u32;
        let mut out = [0u8; 4096];
        let n = fs_getdents64(dir_fd, out.as_mut_ptr() as u64, out.len(), 52).unwrap() as usize;
        let names = parse_dirent_names(&out[..n]);
        assert_eq!(names.len(), 64);
        assert!(names.iter().any(|name| name.as_slice() == b"f0"));
        assert!(names.iter().any(|name| name.as_slice() == b"f3f"));
        assert_eq!(fs_close(dir_fd, 52).unwrap(), 0);
    }

    #[test]
    fn test_fs_symlink_roundtrip_and_following() {
        init_bridge();

        assert_eq!(fs_mkdir(b"/opt", 0, 61).unwrap(), 0);
        assert_eq!(fs_mkdir(b"/opt/bin", 0, 61).unwrap(), 0);
        assert_eq!(fs_mkdir(b"/usr", 0, 61).unwrap(), 0);
        assert_eq!(fs_mkdir(b"/usr/bin", 0, 61).unwrap(), 0);

        let payload = *b"exo-shell";
        let target_path = b"/opt/bin/exo";
        let fd = fs_open(
            target_path,
            open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
            0,
            61,
        )
        .unwrap() as u32;
        assert_eq!(
            fs_write(fd, payload.as_ptr() as u64, payload.len(), 61).unwrap(),
            payload.len() as i64
        );
        assert_eq!(fs_close(fd, 61).unwrap(), 0);

        let target = b"../../opt/bin/exo";
        assert_eq!(fs_symlink(target, b"/usr/bin/exo", 61).unwrap(), 0);

        let mut readlink_out = [0u8; 32];
        let readlink_len = fs_readlink(
            b"/usr/bin/exo",
            readlink_out.as_mut_ptr() as u64,
            readlink_out.len(),
            61,
        )
        .unwrap() as usize;
        assert_eq!(&readlink_out[..readlink_len], target);

        let fd = fs_open(b"/usr/bin/exo", open_flags::O_RDONLY, 0, 61).unwrap() as u32;
        let mut out = [0u8; 16];
        let n = fs_read(fd, out.as_mut_ptr() as u64, out.len(), 61).unwrap() as usize;
        assert_eq!(&out[..n], &payload);
        assert_eq!(fs_close(fd, 61).unwrap(), 0);

        let mut lstat_buf = LinuxStat::default();
        let mut stat_buf = LinuxStat::default();
        assert_eq!(
            fs_lstat(b"/usr/bin/exo", &mut lstat_buf as *mut _ as u64, 61).unwrap(),
            0
        );
        assert_eq!(
            fs_stat(b"/usr/bin/exo", &mut stat_buf as *mut _ as u64, 61).unwrap(),
            0
        );
        assert_eq!(
            lstat_buf.st_mode & STAT_MODE_MASK,
            STAT_MODE_SYMLINK & STAT_MODE_MASK
        );
        assert_eq!(
            stat_buf.st_mode & STAT_MODE_MASK,
            STAT_MODE_FILE & STAT_MODE_MASK
        );
    }

    #[test]
    fn test_fs_symlink_stress_many_relative_links() {
        init_bridge();

        assert_eq!(fs_mkdir(b"/targets", 0, 62).unwrap(), 0);
        assert_eq!(fs_mkdir(b"/links", 0, 62).unwrap(), 0);

        for idx in 0..96u32 {
            let mut target_path = [0u8; 32];
            let target_prefix = b"/targets/t";
            target_path[..target_prefix.len()].copy_from_slice(target_prefix);
            let target_len =
                target_prefix.len() + write_u32_hex(&mut target_path[target_prefix.len()..], idx);

            let fd = fs_open(
                &target_path[..target_len],
                open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
                0,
                62,
            )
            .unwrap() as u32;
            let payload = [idx as u8; 4];
            assert_eq!(
                fs_write(fd, payload.as_ptr() as u64, payload.len(), 62).unwrap(),
                payload.len() as i64
            );
            assert_eq!(fs_close(fd, 62).unwrap(), 0);

            let mut link_path = [0u8; 32];
            let link_prefix = b"/links/l";
            link_path[..link_prefix.len()].copy_from_slice(link_prefix);
            let link_len =
                link_prefix.len() + write_u32_hex(&mut link_path[link_prefix.len()..], idx);

            let mut target_rel = [0u8; 24];
            let rel_prefix = b"../targets/t";
            target_rel[..rel_prefix.len()].copy_from_slice(rel_prefix);
            let rel_len =
                rel_prefix.len() + write_u32_hex(&mut target_rel[rel_prefix.len()..], idx);

            assert_eq!(
                fs_symlink(&target_rel[..rel_len], &link_path[..link_len], 62).unwrap(),
                0
            );

            let fd = fs_open(&link_path[..link_len], open_flags::O_RDONLY, 0, 62).unwrap() as u32;
            let mut out = [0u8; 4];
            assert_eq!(
                fs_read(fd, out.as_mut_ptr() as u64, out.len(), 62).unwrap(),
                out.len() as i64
            );
            assert_eq!(out, [idx as u8; 4]);
            assert_eq!(fs_close(fd, 62).unwrap(), 0);
        }
    }

    #[test]
    fn test_fs_truncate_and_ftruncate_resize_regular_file() {
        init_bridge();

        let path = b"/truncate/data.bin";
        let payload = *b"abcdef";
        let fd = fs_open(path, open_flags::O_CREAT | open_flags::O_RDWR, 0, 71).unwrap() as u32;
        assert_eq!(
            fs_write(fd, payload.as_ptr() as u64, payload.len(), 71).unwrap(),
            payload.len() as i64
        );

        assert_eq!(fs_truncate(path, 3, 71).unwrap(), 0);
        let mut stat_buf = LinuxStat::default();
        assert_eq!(
            fs_stat(path, &mut stat_buf as *mut _ as u64, 71).unwrap(),
            0
        );
        assert_eq!(stat_buf.st_size, 3);

        assert_eq!(fs_ftruncate(fd, 8, 71).unwrap(), 0);
        assert_eq!(fs_lseek(fd, 0, SEEK_SET, 71).unwrap(), 0);
        let mut out = [0xFFu8; 8];
        assert_eq!(
            fs_read(fd, out.as_mut_ptr() as u64, out.len(), 71).unwrap(),
            out.len() as i64
        );
        assert_eq!(&out[..3], b"abc");
        assert_eq!(&out[3..], &[0, 0, 0, 0, 0]);
        assert_eq!(fs_close(fd, 71).unwrap(), 0);
    }

    #[test]
    fn test_fs_truncate_stress_shrink_and_grow_many_files() {
        init_bridge();

        for idx in 0..96u32 {
            let mut path = [0u8; 40];
            let prefix = b"/trunc/f";
            path[..prefix.len()].copy_from_slice(prefix);
            let path_len = prefix.len() + write_u32_hex(&mut path[prefix.len()..], idx);

            let mut payload = [0u8; 24];
            let write_len = 8 + (idx as usize % 16);
            for (off, byte) in payload[..write_len].iter_mut().enumerate() {
                *byte = idx.wrapping_add(off as u32) as u8;
            }

            let fd = fs_open(
                &path[..path_len],
                open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
                0,
                72,
            )
            .unwrap() as u32;
            assert_eq!(
                fs_write(fd, payload.as_ptr() as u64, write_len, 72).unwrap(),
                write_len as i64
            );

            let shrink_len = (idx as u64 % 5) + 1;
            assert_eq!(fs_truncate(&path[..path_len], shrink_len, 72).unwrap(), 0);
            let grow_len = shrink_len + 9;
            assert_eq!(fs_ftruncate(fd, grow_len, 72).unwrap(), 0);

            let mut stat_buf = LinuxStat::default();
            assert_eq!(
                fs_stat(&path[..path_len], &mut stat_buf as *mut _ as u64, 72).unwrap(),
                0
            );
            assert_eq!(stat_buf.st_size, grow_len as i64);
            assert_eq!(fs_close(fd, 72).unwrap(), 0);
        }
    }

    #[test]
    fn test_fs_bridge_large_sequential_write_128_mib() {
        init_bridge();

        let fd = fs_open(
            b"/bench/seq128",
            open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
            0,
            73,
        )
        .unwrap() as u32;
        let block = alloc::vec![0xA5u8; 1024 * 1024];

        let mut idx = 0usize;
        while idx < 128 {
            assert_eq!(
                fs_write(fd, block.as_ptr() as u64, block.len(), 73).unwrap(),
                block.len() as i64
            );
            idx += 1;
        }

        let mut stat = LinuxStat::default();
        assert_eq!(fs_fstat(fd, &mut stat as *mut _ as u64, 73).unwrap(), 0);
        assert_eq!(stat.st_size, 128 * 1024 * 1024);
        assert_eq!(
            fs_lseek(fd, 127 * 1024 * 1024, SEEK_SET, 73).unwrap(),
            127 * 1024 * 1024
        );

        let mut tail = [0u8; 4096];
        assert_eq!(
            fs_read(fd, tail.as_mut_ptr() as u64, tail.len(), 73).unwrap(),
            tail.len() as i64
        );
        assert!(tail.iter().all(|byte| *byte == 0xA5));
        assert_eq!(fs_close(fd, 73).unwrap(), 0);
    }

    #[test]
    fn test_fs_bridge_ftruncate_sparse_extends_128_mib() {
        init_bridge();

        let fd = fs_open(
            b"/bench/sparse128",
            open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
            0,
            74,
        )
        .unwrap() as u32;

        assert_eq!(fs_ftruncate(fd, 128 * 1024 * 1024, 74).unwrap(), 0);

        let mut stat = LinuxStat::default();
        assert_eq!(fs_fstat(fd, &mut stat as *mut _ as u64, 74).unwrap(), 0);
        assert_eq!(stat.st_size, 128 * 1024 * 1024);

        assert_eq!(
            fs_lseek(fd, 127 * 1024 * 1024, SEEK_SET, 74).unwrap(),
            127 * 1024 * 1024
        );
        let mut tail = [0xFFu8; 4096];
        assert_eq!(
            fs_read(fd, tail.as_mut_ptr() as u64, tail.len(), 74).unwrap(),
            tail.len() as i64
        );
        assert!(tail.iter().all(|byte| *byte == 0));
        assert_eq!(fs_close(fd, 74).unwrap(), 0);
    }

    #[test]
    fn test_fs_pipe2_eventfd_and_poll_roundtrip() {
        init_bridge();

        let mut fds = [0i32; 2];
        assert_eq!(fs_pipe2(fds.as_mut_ptr() as u64, 0, 81).unwrap(), 0);

        let payload = *b"pipe-data";
        assert_eq!(
            fs_write(fds[1] as u32, payload.as_ptr() as u64, payload.len(), 81).unwrap(),
            payload.len() as i64
        );

        let mut pollfd = LinuxPollFd {
            fd: fds[0],
            events: POLLIN,
            revents: 0,
        };
        assert_eq!(fs_poll(&mut pollfd as *mut _ as u64, 1, 0, 81).unwrap(), 1);
        assert_ne!(pollfd.revents & POLLIN, 0);

        let mut out = [0u8; 16];
        let n = fs_read(fds[0] as u32, out.as_mut_ptr() as u64, out.len(), 81).unwrap() as usize;
        assert_eq!(&out[..n], &payload);
        assert_eq!(fs_close(fds[0] as u32, 81).unwrap(), 0);
        assert_eq!(fs_close(fds[1] as u32, 81).unwrap(), 0);

        let event_fd = fs_eventfd2(2, 0, 81).unwrap() as u32;
        let add = 3u64;
        assert_eq!(
            fs_write(event_fd, &add as *const _ as u64, size_of::<u64>(), 81).unwrap(),
            size_of::<u64>() as i64
        );
        let mut value = 0u64;
        assert_eq!(
            fs_read(event_fd, &mut value as *mut _ as u64, size_of::<u64>(), 81).unwrap(),
            size_of::<u64>() as i64
        );
        assert_eq!(value, 5);
        assert_eq!(fs_close(event_fd, 81).unwrap(), 0);
    }

    #[test]
    fn test_fs_link_and_inotify_compat_descriptors() {
        init_bridge();

        let fd = fs_open(
            b"/links/source.txt",
            open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
            0,
            83,
        )
        .unwrap() as u32;
        let payload = *b"hard-link";
        assert_eq!(
            fs_write(fd, payload.as_ptr() as u64, payload.len(), 83).unwrap(),
            payload.len() as i64
        );
        assert_eq!(fs_close(fd, 83).unwrap(), 0);

        assert_eq!(
            fs_link(b"/links/source.txt", b"/links/alias.txt", 83).unwrap(),
            0
        );
        let alias_fd = fs_open(b"/links/alias.txt", open_flags::O_RDONLY, 0, 83).unwrap() as u32;
        let mut out = [0u8; 16];
        let n = fs_read(alias_fd, out.as_mut_ptr() as u64, out.len(), 83).unwrap() as usize;
        assert_eq!(&out[..n], &payload);
        assert_eq!(fs_close(alias_fd, 83).unwrap(), 0);

        let inotify_fd = fs_inotify_init1(O_NONBLOCK | O_CLOEXEC, 83).unwrap() as u32;
        let mut pollfd = LinuxPollFd {
            fd: inotify_fd as i32,
            events: POLLIN,
            revents: 0,
        };
        assert_eq!(fs_poll(&mut pollfd as *mut _ as u64, 1, 0, 83).unwrap(), 0);
        assert_eq!(fs_close(inotify_fd, 83).unwrap(), 0);
    }

    #[test]
    fn test_fs_phase2_compat_splice_vmsplice_socketpair_mknod() {
        init_bridge();

        assert_eq!(
            fs_mknod(b"/nodes/fifo0", S_IFIFO | 0o644, 0, 84).unwrap(),
            0
        );
        let mut stat = LinuxStat::default();
        assert_eq!(
            fs_stat(b"/nodes/fifo0", &mut stat as *mut _ as u64, 84).unwrap(),
            0
        );

        let mut pipe_a = [0i32; 2];
        let mut pipe_b = [0i32; 2];
        assert_eq!(fs_pipe2(pipe_a.as_mut_ptr() as u64, 0, 84).unwrap(), 0);
        assert_eq!(fs_pipe2(pipe_b.as_mut_ptr() as u64, 0, 84).unwrap(), 0);

        let vm_payload = *b"vmsplice";
        let iov = LinuxIovec {
            iov_base: vm_payload.as_ptr() as u64,
            iov_len: vm_payload.len() as u64,
        };
        assert_eq!(
            fs_vmsplice(pipe_a[1] as u32, &iov as *const _ as u64, 1, 0, 84).unwrap(),
            vm_payload.len() as i64
        );
        assert_eq!(
            fs_tee(pipe_a[0] as u32, pipe_b[1] as u32, vm_payload.len(), 0, 84).unwrap(),
            vm_payload.len() as i64
        );

        let mut tee_out = [0u8; 16];
        let n = fs_read(
            pipe_b[0] as u32,
            tee_out.as_mut_ptr() as u64,
            tee_out.len(),
            84,
        )
        .unwrap() as usize;
        assert_eq!(&tee_out[..n], &vm_payload);

        let file_fd = fs_open(
            b"/splice/out.bin",
            open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
            0,
            84,
        )
        .unwrap() as u32;
        assert_eq!(
            fs_splice(pipe_a[0] as u32, 0, file_fd, 0, vm_payload.len(), 0, 84).unwrap(),
            vm_payload.len() as i64
        );
        assert_eq!(fs_lseek(file_fd, 0, SEEK_SET, 84).unwrap(), 0);
        let mut file_out = [0u8; 16];
        let n =
            fs_read(file_fd, file_out.as_mut_ptr() as u64, file_out.len(), 84).unwrap() as usize;
        assert_eq!(&file_out[..n], &vm_payload);

        let mut sv = [0i32; 2];
        assert_eq!(
            fs_socketpair(1, 1, 0, sv.as_mut_ptr() as u64, 84).unwrap(),
            0
        );
        let sock_payload = *b"sock";
        assert_eq!(
            fs_write(
                sv[0] as u32,
                sock_payload.as_ptr() as u64,
                sock_payload.len(),
                84
            )
            .unwrap(),
            sock_payload.len() as i64
        );
        let mut sock_out = [0u8; 8];
        let n = fs_read(
            sv[1] as u32,
            sock_out.as_mut_ptr() as u64,
            sock_out.len(),
            84,
        )
        .unwrap() as usize;
        assert_eq!(&sock_out[..n], &sock_payload);

        assert_eq!(fs_close(file_fd, 84).unwrap(), 0);
        assert_eq!(fs_close(pipe_a[0] as u32, 84).unwrap(), 0);
        assert_eq!(fs_close(pipe_a[1] as u32, 84).unwrap(), 0);
        assert_eq!(fs_close(pipe_b[0] as u32, 84).unwrap(), 0);
        assert_eq!(fs_close(pipe_b[1] as u32, 84).unwrap(), 0);
        assert_eq!(fs_close(sv[0] as u32, 84).unwrap(), 0);
        assert_eq!(fs_close(sv[1] as u32, 84).unwrap(), 0);
    }

    #[test]
    fn test_fs_copy_range_sendfile_statx_and_cwd_compat() {
        init_bridge();

        let src = fs_open(
            b"/copy/src.bin",
            open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
            0,
            82,
        )
        .unwrap() as u32;
        let dst = fs_open(
            b"/copy/dst.bin",
            open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
            0,
            82,
        )
        .unwrap() as u32;
        let payload = *b"translation-layer";
        assert_eq!(
            fs_write(src, payload.as_ptr() as u64, payload.len(), 82).unwrap(),
            payload.len() as i64
        );
        assert_eq!(fs_lseek(src, 0, SEEK_SET, 82).unwrap(), 0);
        assert_eq!(
            fs_copy_file_range(src, 0, dst, 0, payload.len(), 0, 82).unwrap(),
            payload.len() as i64
        );
        assert_eq!(fs_lseek(dst, 0, SEEK_SET, 82).unwrap(), 0);
        let mut out = [0u8; 32];
        let n = fs_read(dst, out.as_mut_ptr() as u64, payload.len(), 82).unwrap() as usize;
        assert_eq!(&out[..n], &payload);

        let mut statx = LinuxStatx::default();
        assert_eq!(
            fs_statx(
                AT_FDCWD,
                b"/copy/dst.bin",
                0,
                0,
                &mut statx as *mut _ as u64,
                82
            )
            .unwrap(),
            0
        );
        assert_eq!(statx.stx_size, payload.len() as u64);

        assert_eq!(fs_fallocate(dst, 0, 0, 64, 82).unwrap(), 0);
        let mut stat = LinuxStat::default();
        assert_eq!(fs_fstat(dst, &mut stat as *mut _ as u64, 82).unwrap(), 0);
        assert_eq!(stat.st_size, 64);

        let mut cwd = [0u8; 4];
        assert_eq!(
            fs_getcwd(cwd.as_mut_ptr() as u64, cwd.len(), 82).unwrap(),
            2
        );
        assert_eq!(&cwd[..2], b"/\0");
        assert_eq!(
            fs_sync_file_range(dst, 0, 64, SYNC_FILE_RANGE_WRITE, 82).unwrap(),
            0
        );
        assert_eq!(fs_close(src, 82).unwrap(), 0);
        assert_eq!(fs_close(dst, 82).unwrap(), 0);
    }

    fn write_u32_hex(dst: &mut [u8], value: u32) -> usize {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut shift = 28u32;
        let mut out = 0usize;
        let mut started = false;

        loop {
            let nibble = ((value >> shift) & 0xF) as usize;
            if nibble != 0 || started || shift == 0 {
                dst[out] = HEX[nibble];
                out += 1;
                started = true;
            }
            if shift == 0 {
                break;
            }
            shift -= 4;
        }

        out
    }
}
