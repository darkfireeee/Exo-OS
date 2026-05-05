#![no_std]
#![no_main]
// The translation catalog intentionally exposes more services than the boot path
// calls directly; every entry is consumed by IPC/syscall dispatch as it matures.
#![allow(dead_code, unused_imports)]

//! # vfs_server — PID 3, Virtual File System namespace
//!
//! Responsabilités :
//!   - Monter ExoFS (vraie FS) sur `/`
//!   - Monter les pseudo-filesystems : `/proc`, `/sys`, `/dev`
//!   - Résoudre les chemins en BlobId via SYS_EXOFS_PATH_RESOLVE (500)
//!   - Servir les requêtes de montage/unmount depuis les autres servers
//!   - Maintenir la table de montages globale (max 32 points)
//!
//! ## Protocole IPC (msg_type)
//! - VFS_MOUNT    (0) : monter un FS (device, mountpoint, fstype, flags)
//! - VFS_UMOUNT   (1) : démonter un point de montage
//! - VFS_RESOLVE  (2) : résoudre un chemin -> blob_id bas 64 bits
//! - VFS_OPEN     (3) : ouvrir un chemin -> fd dans le namespace appelant
//! - VFS_CLOSE..FSYNC (4..14) : operations POSIX de base, deleguees au kernel
//!
//! ## Syscalls utilisés
//! - SYS_EXOFS_PATH_RESOLVE = 500 (path, len, flags, out, _, rights)
//! - SYS_EXOFS_OPEN_BY_PATH = 519 (path, flags, mode, _, _, rights)
//! - SYS_IPC_REGISTER = 304, SYS_IPC_RECV = 301, SYS_IPC_SEND = 300

use core::cell::UnsafeCell;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU32, Ordering};
use exo_syscall_abi as syscall;

mod compat;
mod ops;
mod translation_layer;

// ── Table de montages ─────────────────────────────────────────────────────────

/// Types de pseudo-FS supportés.
#[repr(u8)]
#[derive(Copy, Clone, PartialEq)]
enum FsType {
    None = 0,
    ExoFs = 1,
    ProcFs = 2,
    SysFs = 3,
    DevFs = 4,
}

impl FsType {
    fn from_wire(value: u8) -> Option<Self> {
        match value {
            compat::FS_EXOFS => Some(Self::ExoFs),
            compat::FS_PROCFS => Some(Self::ProcFs),
            compat::FS_SYSFS => Some(Self::SysFs),
            compat::FS_DEVFS => Some(Self::DevFs),
            _ => None,
        }
    }
}

/// Entrée de la table de montages.
#[derive(Copy, Clone)]
#[allow(dead_code)]
struct MountEntry {
    fs_type: FsType,
    /// Hash FNV-32 du chemin de montage (ex : hash("/proc")).
    path_hash: u32,
    /// BlobId du répertoire racine ExoFS (0 pour pseudo-FS).
    root_blob: u64,
    active: bool,
}

impl MountEntry {
    const fn empty() -> Self {
        Self {
            fs_type: FsType::None,
            path_hash: 0,
            root_blob: 0,
            active: false,
        }
    }
}

static MOUNT_COUNT: AtomicU32 = AtomicU32::new(0);
const MAX_MOUNTS: usize = 32;

struct MountTable(UnsafeCell<[MountEntry; MAX_MOUNTS]>);

unsafe impl Sync for MountTable {}

static MOUNTS: MountTable = MountTable(UnsafeCell::new([MountEntry::empty(); MAX_MOUNTS]));
static IPC_RECV_TIMEOUTS: AtomicU32 = AtomicU32::new(0);

fn fnv32(s: &[u8]) -> u32 {
    let mut h: u32 = 2166136261;
    for &b in s {
        h = h.wrapping_mul(16777619).wrapping_add(b as u32);
    }
    h
}

// ── Messages IPC ─────────────────────────────────────────────────────────────

const IPC_RECV_TIMEOUT_MS: u64 = 5_000;
const IPC_FLAG_TIMEOUT: u64 = syscall::IPC_FLAG_TIMEOUT;
const ETIMEDOUT: i64 = syscall::ETIMEDOUT;

#[repr(C)]
struct VfsRequest {
    sender_pid: u32,
    msg_type: u32,
    payload: [u8; ops::PATH_PAYLOAD_MAX],
}

#[repr(C)]
struct VfsReply {
    status: i64,
    blob_id: u64,
    fd: i64,
    _pad: [u8; 40],
}

fn handle_mount(sender_pid: u32, payload: &[u8]) -> VfsReply {
    if sender_pid != 1 {
        return VfsReply {
            status: syscall::EPERM,
            blob_id: 0,
            fd: -1,
            _pad: [0; 40],
        };
    }

    // payload[0] = fstype u8, payload[1..5] = flags u32 LE,
    // payload[5..13] = root_blob u64 LE, payload[13..] = chemin null-terminated
    if payload.len() < 14 {
        return VfsReply {
            status: syscall::EINVAL,
            blob_id: 0,
            fd: -1,
            _pad: [0; 40],
        }; // -EINVAL
    }
    let fstype = payload[0];
    let root_blob = u64::from_le_bytes([
        payload[5],
        payload[6],
        payload[7],
        payload[8],
        payload[9],
        payload[10],
        payload[11],
        payload[12],
    ]);
    let path = &payload[13..];
    // Trouver le null terminator
    let path_len = path.iter().position(|&b| b == 0).unwrap_or(path.len());
    let path_hash = fnv32(&path[..path_len]);

    let fs = match FsType::from_wire(fstype) {
        Some(fs_type) => fs_type,
        None => {
            return VfsReply {
                status: syscall::EINVAL,
                blob_id: 0,
                fd: -1,
                _pad: [0; 40],
            }
        }
    };

    let mut free_idx = None;
    unsafe {
        let mounts = &mut *MOUNTS.0.get();
        for i in 0..MAX_MOUNTS {
            if mounts[i].active && mounts[i].path_hash == path_hash {
                mounts[i] = MountEntry {
                    fs_type: fs,
                    path_hash,
                    root_blob,
                    active: true,
                };
                return VfsReply {
                    status: 0,
                    blob_id: root_blob,
                    fd: i as i64,
                    _pad: [0; 40],
                };
            }
            if free_idx.is_none() && !mounts[i].active {
                free_idx = Some(i);
            }
        }
    }

    let idx = match free_idx {
        Some(i) => i,
        None => {
            return VfsReply {
                status: syscall::ENOSPC,
                blob_id: 0,
                fd: -1,
                _pad: [0; 40],
            }
        }
    };

    unsafe {
        let mounts = &mut *MOUNTS.0.get();
        mounts[idx] = MountEntry {
            fs_type: fs,
            path_hash,
            root_blob,
            active: true,
        };
    }
    MOUNT_COUNT.fetch_add(1, Ordering::AcqRel);

    VfsReply {
        status: 0,
        blob_id: root_blob,
        fd: idx as i64,
        _pad: [0; 40],
    }
}

fn handle_resolve(payload: &[u8]) -> VfsReply {
    let mut path = [0u8; ops::PATH_PAYLOAD_MAX + 1];
    let path_len = match ops::path_payload_to_cstr(payload, &mut path) {
        Ok(len) => len,
        Err(status) => {
            return VfsReply {
                status,
                blob_id: 0,
                fd: -1,
                _pad: [0; 40],
            }
        }
    };
    let mut resolved = syscall::ExofsPathResolveResult::default();

    let rc = unsafe {
        syscall::exofs_path_resolve_raw(
            path.as_ptr() as u64,
            path_len as u64,
            0,
            &mut resolved,
            ops::EXOFS_READ_RIGHTS,
        )
    };

    if rc < 0 {
        VfsReply {
            status: rc,
            blob_id: 0,
            fd: -1,
            _pad: [0; 40],
        }
    } else {
        VfsReply {
            status: 0,
            blob_id: resolved.blob_id_low64(),
            fd: -1,
            _pad: [0; 40],
        }
    }
}

fn handle_open(payload: &[u8]) -> VfsReply {
    // Format préféré: payload[0..4] = flags u32 LE, payload[4..] = chemin C.
    // Compatibilité: si payload[0] ressemble à un chemin, flags=O_RDONLY.
    let (flags, path_payload) = match ops::open_payload_parts(payload) {
        Ok(parts) => parts,
        Err(status) => {
            return VfsReply {
                status,
                blob_id: 0,
                fd: -1,
                _pad: [0; 40],
            }
        }
    };

    let mut path = [0u8; ops::PATH_PAYLOAD_MAX + 1];
    let path_len = match ops::path_payload_to_cstr(path_payload, &mut path) {
        Ok(len) => len,
        Err(status) => {
            return VfsReply {
                status,
                blob_id: 0,
                fd: -1,
                _pad: [0; 40],
            }
        }
    };

    let rights = ops::exofs_rights_for_open(flags);

    let fd = unsafe { syscall::exofs_open_by_path_raw(path.as_ptr() as u64, flags, 0, rights) };

    if fd < 0 {
        VfsReply {
            status: fd,
            blob_id: 0,
            fd: -1,
            _pad: [0; 40],
        }
    } else {
        let mut resolved = syscall::ExofsPathResolveResult::default();
        let resolve_rc = unsafe {
            syscall::exofs_path_resolve_raw(
                path.as_ptr() as u64,
                path_len as u64,
                0,
                &mut resolved,
                ops::EXOFS_READ_RIGHTS,
            )
        };
        let blob_id = if resolve_rc < 0 {
            0
        } else {
            resolved.blob_id_low64()
        };
        VfsReply {
            status: 0,
            blob_id,
            fd,
            _pad: [0; 40],
        }
    }
}

fn handle_umount(sender_pid: u32, payload: &[u8]) -> VfsReply {
    if sender_pid != 1 {
        return VfsReply {
            status: syscall::EPERM,
            blob_id: 0,
            fd: -1,
            _pad: [0; 40],
        };
    }

    let path_len = payload
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(payload.len());
    if path_len == 0 {
        return VfsReply {
            status: syscall::EINVAL,
            blob_id: 0,
            fd: -1,
            _pad: [0; 40],
        };
    }

    let path_hash = fnv32(&payload[..path_len]);
    unsafe {
        let mounts = &mut *MOUNTS.0.get();
        for i in 0..MAX_MOUNTS {
            if mounts[i].active && mounts[i].path_hash == path_hash {
                mounts[i] = MountEntry::empty();
                MOUNT_COUNT
                    .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |count| {
                        Some(count.saturating_sub(1))
                    })
                    .ok();
                return VfsReply {
                    status: 0,
                    blob_id: 0,
                    fd: i as i64,
                    _pad: [0; 40],
                };
            }
        }
    }

    VfsReply {
        status: syscall::ENOENT,
        blob_id: 0,
        fd: -1,
        _pad: [0; 40],
    }
}

fn reply_status(status: i64) -> VfsReply {
    VfsReply {
        status,
        blob_id: 0,
        fd: -1,
        _pad: [0; 40],
    }
}

fn reply_count(status: i64, count: i64) -> VfsReply {
    VfsReply {
        status,
        blob_id: 0,
        fd: count,
        _pad: [0; 40],
    }
}

fn handle_close(payload: &[u8]) -> VfsReply {
    let fd = match ops::read_u64(payload, 0) {
        Ok(fd) => fd,
        Err(status) => return reply_status(status),
    };
    let rc = unsafe { syscall::syscall1(syscall::SYS_CLOSE, fd) };
    if rc < 0 {
        reply_status(rc)
    } else {
        reply_count(0, rc)
    }
}

fn handle_read(payload: &[u8]) -> VfsReply {
    let fd = match ops::read_u64(payload, 0) {
        Ok(fd) => fd,
        Err(status) => return reply_status(status),
    };
    let buf = match ops::read_u64(payload, 8) {
        Ok(buf) => buf,
        Err(status) => return reply_status(status),
    };
    let len = match ops::read_u64(payload, 16) {
        Ok(len) => len,
        Err(status) => return reply_status(status),
    };
    let rc = unsafe { syscall::syscall3(syscall::SYS_READ, fd, buf, len) };
    if rc < 0 {
        reply_status(rc)
    } else {
        reply_count(0, rc)
    }
}

fn handle_write(payload: &[u8]) -> VfsReply {
    let fd = match ops::read_u64(payload, 0) {
        Ok(fd) => fd,
        Err(status) => return reply_status(status),
    };
    let buf = match ops::read_u64(payload, 8) {
        Ok(buf) => buf,
        Err(status) => return reply_status(status),
    };
    let len = match ops::read_u64(payload, 16) {
        Ok(len) => len,
        Err(status) => return reply_status(status),
    };
    let rc = unsafe { syscall::syscall3(syscall::SYS_WRITE, fd, buf, len) };
    if rc < 0 {
        reply_status(rc)
    } else {
        reply_count(0, rc)
    }
}

fn handle_path_mode(payload: &[u8], nr: u64) -> VfsReply {
    let mode = match ops::read_u64(payload, 0) {
        Ok(mode) => mode,
        Err(status) => return reply_status(status),
    };
    let mut path = [0u8; ops::PATH_PAYLOAD_MAX + 1];
    let path_len = match ops::path_payload_to_cstr(&payload[8..], &mut path) {
        Ok(len) => len,
        Err(status) => return reply_status(status),
    };
    let _ = path_len;
    let rc = unsafe { syscall::syscall2(nr, path.as_ptr() as u64, mode) };
    reply_status(if rc < 0 { rc } else { 0 })
}

fn handle_path_only(payload: &[u8], nr: u64) -> VfsReply {
    let mut path = [0u8; ops::PATH_PAYLOAD_MAX + 1];
    let path_len = match ops::path_payload_to_cstr(payload, &mut path) {
        Ok(len) => len,
        Err(status) => return reply_status(status),
    };
    let _ = path_len;
    let rc = unsafe { syscall::syscall1(nr, path.as_ptr() as u64) };
    reply_status(if rc < 0 { rc } else { 0 })
}

fn handle_stat(payload: &[u8]) -> VfsReply {
    let stat_ptr = match ops::read_u64(payload, 0) {
        Ok(ptr) => ptr,
        Err(status) => return reply_status(status),
    };
    let mut path = [0u8; ops::PATH_PAYLOAD_MAX + 1];
    let path_len = match ops::path_payload_to_cstr(&payload[8..], &mut path) {
        Ok(len) => len,
        Err(status) => return reply_status(status),
    };
    let _ = path_len;
    let rc = unsafe { syscall::syscall2(syscall::SYS_STAT, path.as_ptr() as u64, stat_ptr) };
    reply_status(if rc < 0 { rc } else { 0 })
}

fn handle_getdents(payload: &[u8]) -> VfsReply {
    let fd = match ops::read_u64(payload, 0) {
        Ok(fd) => fd,
        Err(status) => return reply_status(status),
    };
    let buf = match ops::read_u64(payload, 8) {
        Ok(buf) => buf,
        Err(status) => return reply_status(status),
    };
    let len = match ops::read_u64(payload, 16) {
        Ok(len) => len,
        Err(status) => return reply_status(status),
    };
    let rc = unsafe { syscall::syscall3(syscall::SYS_GETDENTS64, fd, buf, len) };
    if rc < 0 {
        reply_status(rc)
    } else {
        reply_count(0, rc)
    }
}

fn handle_rename(payload: &[u8]) -> VfsReply {
    let old_len = match ops::read_u64(payload, 0) {
        Ok(len) if len > 0 && len <= ops::PATH_PAYLOAD_MAX as u64 => len as usize,
        Ok(_) => return reply_status(syscall::EINVAL),
        Err(status) => return reply_status(status),
    };
    let old_start = 8usize;
    let new_start = match old_start
        .checked_add(old_len)
        .and_then(|v| v.checked_add(1))
    {
        Some(start) if start < payload.len() => start,
        _ => return reply_status(syscall::EINVAL),
    };
    let mut old_path = [0u8; ops::PATH_PAYLOAD_MAX + 1];
    let mut new_path = [0u8; ops::PATH_PAYLOAD_MAX + 1];
    let old = &payload[old_start..old_start + old_len];
    old_path[..old_len].copy_from_slice(old);
    old_path[old_len] = 0;
    let new_len = match ops::path_payload_to_cstr(&payload[new_start..], &mut new_path) {
        Ok(len) => len,
        Err(status) => return reply_status(status),
    };
    let _ = new_len;
    let rc = unsafe {
        syscall::syscall2(
            syscall::SYS_RENAME,
            old_path.as_ptr() as u64,
            new_path.as_ptr() as u64,
        )
    };
    reply_status(if rc < 0 { rc } else { 0 })
}

fn handle_truncate(payload: &[u8]) -> VfsReply {
    let len = match ops::read_u64(payload, 0) {
        Ok(len) => len,
        Err(status) => return reply_status(status),
    };
    let mut path = [0u8; ops::PATH_PAYLOAD_MAX + 1];
    let path_len = match ops::path_payload_to_cstr(&payload[8..], &mut path) {
        Ok(len) => len,
        Err(status) => return reply_status(status),
    };
    let _ = path_len;
    let rc = unsafe { syscall::syscall2(syscall::SYS_TRUNCATE, path.as_ptr() as u64, len) };
    reply_status(if rc < 0 { rc } else { 0 })
}

fn handle_fsync(payload: &[u8]) -> VfsReply {
    let fd = match ops::read_u64(payload, 0) {
        Ok(fd) => fd,
        Err(status) => return reply_status(status),
    };
    let rc = unsafe { syscall::syscall1(syscall::SYS_FSYNC, fd) };
    reply_status(if rc < 0 { rc } else { 0 })
}

fn handle_request(req: &VfsRequest) -> VfsReply {
    match req.msg_type {
        ops::VFS_MOUNT => handle_mount(req.sender_pid, &req.payload),
        ops::VFS_RESOLVE => handle_resolve(&req.payload),
        ops::VFS_OPEN => handle_open(&req.payload),
        ops::VFS_UMOUNT => handle_umount(req.sender_pid, &req.payload),
        ops::VFS_CLOSE => handle_close(&req.payload),
        ops::VFS_READ => handle_read(&req.payload),
        ops::VFS_WRITE => handle_write(&req.payload),
        ops::VFS_STAT => handle_stat(&req.payload),
        ops::VFS_GETDENTS => handle_getdents(&req.payload),
        ops::VFS_MKDIR => handle_path_mode(&req.payload, syscall::SYS_MKDIR),
        ops::VFS_UNLINK => handle_path_only(&req.payload, syscall::SYS_UNLINK),
        ops::VFS_RMDIR => handle_path_only(&req.payload, syscall::SYS_RMDIR),
        ops::VFS_RENAME => handle_rename(&req.payload),
        ops::VFS_TRUNCATE => handle_truncate(&req.payload),
        ops::VFS_FSYNC => handle_fsync(&req.payload),
        _ => VfsReply {
            status: syscall::EINVAL,
            blob_id: 0,
            fd: -1,
            _pad: [0; 40],
        },
    }
}

/// Monte les pseudo-filesystems de base au démarrage.
fn mount_default_namespaces() {
    debug_assert!(compat::magic_values_are_layered());
    debug_assert!(translation_layer::translation_contract_is_sane());

    unsafe {
        let mounts = &mut *MOUNTS.0.get();
        for (idx, spec) in compat::DEFAULT_PSEUDO_MOUNTS.iter().enumerate() {
            if let Some(fs_type) = FsType::from_wire(spec.fs_type) {
                mounts[idx] = MountEntry {
                    fs_type,
                    path_hash: fnv32(spec.path),
                    root_blob: spec.root_blob,
                    active: true,
                };
            }
        }
    }
    MOUNT_COUNT.store(
        compat::DEFAULT_PSEUDO_MOUNTS.len() as u32,
        Ordering::Release,
    );
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // ── 1. Monter les pseudo-FS de base ───────────────────────────────────────
    mount_default_namespaces();

    // ── 2. S'enregistrer auprès de l'ipc_router ──────────────────────────────
    let name = b"vfs_server";
    let _ = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            3u64, // endpoint_id = PID 3
        )
    };

    // ── 3. Boucle de service ──────────────────────────────────────────────────
    let mut req = VfsRequest {
        sender_pid: 0,
        msg_type: 0,
        payload: [0u8; ops::PATH_PAYLOAD_MAX],
    };

    loop {
        let r = unsafe {
            syscall::syscall3(
                syscall::SYS_IPC_RECV,
                &mut req as *mut VfsRequest as u64,
                core::mem::size_of::<VfsRequest>() as u64,
                IPC_FLAG_TIMEOUT | IPC_RECV_TIMEOUT_MS,
            )
        };

        if r == ETIMEDOUT {
            IPC_RECV_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        if r < 0 {
            continue;
        }

        let reply = handle_request(&req);

        let _ = unsafe {
            syscall::syscall6(
                syscall::SYS_IPC_SEND,
                req.sender_pid as u64,
                &reply as *const VfsReply as u64,
                core::mem::size_of::<VfsReply>() as u64,
                0,
                0,
                0,
            )
        };
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nostack, nomem));
        }
    }
}
