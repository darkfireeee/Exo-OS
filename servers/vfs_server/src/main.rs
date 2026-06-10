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
use spin::Mutex as SpinMutex;

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

// FIX-VFS-MOUNT (ANALYSE_SERVERS_EXOOS §P2) : l'ancienne MountTable utilisait
// un UnsafeCell avec unsafe impl Sync — aucune protection contre les accès
// concurrents. Si deux requêtes IPC arrivaient simultanément (scheduler multi-CPU),
// la table pouvait être corrompue silencieusement.
//
// Correction : spin::Mutex enveloppant le tableau. Le vfs_server est mono-thread
// dans la boucle principale, donc le verrou n'est jamais contenu (zero-contention
// en pratique), mais il documente l'invariant et protège contre de futures
// évolutions multi-thread.
static MOUNTS: SpinMutex<[MountEntry; MAX_MOUNTS]> =
    SpinMutex::new([MountEntry::empty(); MAX_MOUNTS]);
static IPC_RECV_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
const CROSS_PROCESS_CHUNK: usize = 4096;
const LINUX_STAT_SIZE: usize = 144;
const SERVER_ENDPOINT_ID: u64 = 3;

#[inline]
fn boot_log(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    unsafe {
        let _ = syscall::syscall3(
            syscall::SYS_EXO_LOG,
            bytes.as_ptr() as u64,
            bytes.len() as u64,
            1,
        );
    }
}

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

const _: () = assert!(core::mem::size_of::<VfsRequest>() == syscall::IPC_ENVELOPE_SIZE);
const _: () = assert!(core::mem::offset_of!(VfsRequest, payload) == syscall::IPC_HEADER_SIZE);

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
    {
        let mut guard = MOUNTS.lock();
        let mounts = &mut *guard;
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

    {
        let mut guard = MOUNTS.lock();
        let mounts = &mut *guard;
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
    {
        let mut guard = MOUNTS.lock();
        let mounts = &mut *guard;
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

fn copy_from_sender(sender_pid: u32, src: u64, dst: *mut u8, len: usize) -> i64 {
    unsafe {
        syscall::syscall5(
            syscall::SYS_EXO_MEM_COPY_FROM_PID,
            sender_pid as u64,
            src,
            dst as u64,
            len as u64,
            0,
        )
    }
}

fn copy_to_sender(sender_pid: u32, dst: u64, src: *const u8, len: usize) -> i64 {
    unsafe {
        syscall::syscall5(
            syscall::SYS_EXO_MEM_COPY_TO_PID,
            sender_pid as u64,
            dst,
            src as u64,
            len as u64,
            0,
        )
    }
}

fn handle_read(sender_pid: u32, payload: &[u8]) -> VfsReply {
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

    let mut scratch = [0u8; CROSS_PROCESS_CHUNK];
    let mut done = 0u64;
    while done < len {
        let chunk = (len - done).min(scratch.len() as u64) as usize;
        let rc = unsafe {
            syscall::syscall3(
                syscall::SYS_READ,
                fd,
                scratch.as_mut_ptr() as u64,
                chunk as u64,
            )
        };
        if rc < 0 {
            return if done == 0 {
                reply_status(rc)
            } else {
                reply_count(0, done as i64)
            };
        }
        if rc == 0 {
            break;
        }
        let copied = copy_to_sender(sender_pid, buf + done, scratch.as_ptr(), rc as usize);
        if copied < 0 {
            return if done == 0 {
                reply_status(copied)
            } else {
                reply_count(0, done as i64)
            };
        }
        done = done.saturating_add(copied as u64);
        if copied < rc {
            break;
        }
        if rc < chunk as i64 {
            break;
        }
    }
    reply_count(0, done as i64)
}

fn handle_write(sender_pid: u32, payload: &[u8]) -> VfsReply {
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

    let mut scratch = [0u8; CROSS_PROCESS_CHUNK];
    let mut done = 0u64;
    while done < len {
        let chunk = (len - done).min(scratch.len() as u64) as usize;
        let copied = copy_from_sender(sender_pid, buf + done, scratch.as_mut_ptr(), chunk);
        if copied < 0 {
            return if done == 0 {
                reply_status(copied)
            } else {
                reply_count(0, done as i64)
            };
        }
        if copied == 0 {
            break;
        }
        let rc = unsafe {
            syscall::syscall3(
                syscall::SYS_WRITE,
                fd,
                scratch.as_ptr() as u64,
                copied as u64,
            )
        };
        if rc < 0 {
            return if done == 0 {
                reply_status(rc)
            } else {
                reply_count(0, done as i64)
            };
        }
        done = done.saturating_add(rc as u64);
        if rc < copied {
            break;
        }
    }
    reply_count(0, done as i64)
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

fn handle_stat(sender_pid: u32, payload: &[u8]) -> VfsReply {
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
    let mut stat_buf = [0u8; LINUX_STAT_SIZE];
    let rc = unsafe {
        syscall::syscall2(
            syscall::SYS_STAT,
            path.as_ptr() as u64,
            stat_buf.as_mut_ptr() as u64,
        )
    };
    if rc < 0 {
        return reply_status(rc);
    }
    let copied = copy_to_sender(sender_pid, stat_ptr, stat_buf.as_ptr(), stat_buf.len());
    reply_status(if copied < 0 { copied } else { 0 })
}

fn handle_getdents(sender_pid: u32, payload: &[u8]) -> VfsReply {
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

    let mut scratch = [0u8; CROSS_PROCESS_CHUNK];
    let mut done = 0u64;
    while done < len {
        let chunk = (len - done).min(scratch.len() as u64) as usize;
        let rc = unsafe {
            syscall::syscall3(
                syscall::SYS_GETDENTS64,
                fd,
                scratch.as_mut_ptr() as u64,
                chunk as u64,
            )
        };
        if rc < 0 {
            return if done == 0 {
                reply_status(rc)
            } else {
                reply_count(0, done as i64)
            };
        }
        if rc == 0 {
            break;
        }
        let copied = copy_to_sender(sender_pid, buf + done, scratch.as_ptr(), rc as usize);
        if copied < 0 {
            return if done == 0 {
                reply_status(copied)
            } else {
                reply_count(0, done as i64)
            };
        }
        done = done.saturating_add(copied as u64);
        if copied < rc || rc < chunk as i64 {
            break;
        }
    }
    reply_count(0, done as i64)
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

// FIX-APP-06 (Security_Application_Audit §GAP-06) : garde d'accès VFS.
// En v0.2.0, validation basée sur les PIDs de confiance Ring1.
// Règle : PIDs 2..9 (serveurs Ring1 non-storage) ont accès READ seul sauf PID 1,3.
// PID 1 (init) et PID 3 (vfs lui-même) peuvent toujours écrire.
// PIDs > 10 (exosh, apps) peuvent lire et écrire leurs propres fichiers.
#[inline]
fn check_vfs_write_access(sender_pid: u32) -> bool {
    const WRITE_ALLOWED: &[u32] = &[1, 3];
    WRITE_ALLOWED.contains(&sender_pid) || sender_pid >= 10
}

fn handle_request(req: &VfsRequest) -> VfsReply {
    match req.msg_type {
        ops::VFS_MOUNT => handle_mount(req.sender_pid, &req.payload),
        ops::VFS_RESOLVE => handle_resolve(&req.payload),
        ops::VFS_OPEN => handle_open(&req.payload),
        ops::VFS_UMOUNT => handle_umount(req.sender_pid, &req.payload),
        ops::VFS_CLOSE => handle_close(&req.payload),
        ops::VFS_READ => handle_read(req.sender_pid, &req.payload),
        ops::VFS_WRITE => {
            // FIX-APP-06: vérification write access
            if !check_vfs_write_access(req.sender_pid) {
                return VfsReply { status: syscall::EACCES, blob_id: 0, fd: -1, _pad: [0; 40] };
            }
            handle_write(req.sender_pid, &req.payload)
        }
        ops::VFS_STAT => handle_stat(req.sender_pid, &req.payload),
        ops::VFS_GETDENTS => handle_getdents(req.sender_pid, &req.payload),
        // FIX-APP-06 (complément) : mkdir/unlink/rmdir/rename/truncate sont des
        // mutations au même titre que write — la garde d'accès ne couvrait que
        // VFS_WRITE, laissant les services read-only détruire des fichiers.
        ops::VFS_MKDIR | ops::VFS_UNLINK | ops::VFS_RMDIR | ops::VFS_RENAME
        | ops::VFS_TRUNCATE
            if !check_vfs_write_access(req.sender_pid) =>
        {
            VfsReply { status: syscall::EACCES, blob_id: 0, fd: -1, _pad: [0; 40] }
        }
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

    {
        let mut guard = MOUNTS.lock();
        let mounts = &mut *guard;
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
    let register_rc = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            SERVER_ENDPOINT_ID,
        )
    };
    if register_rc < 0 {
        boot_log(b"vfs_server: register failed\n");
        halt_forever();
    }
    boot_log(b"vfs_server: registered\n");

    // ── 3. Boucle de service ──────────────────────────────────────────────────
    let mut req = VfsRequest {
        sender_pid: 0,
        msg_type: 0,
        payload: [0u8; ops::PATH_PAYLOAD_MAX],
    };

    loop {
        let r = unsafe {
            syscall::syscall4(
                syscall::SYS_IPC_RECV,
                SERVER_ENDPOINT_ID,
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
    boot_log(b"vfs_server: panic\n");
    halt_forever();
}

fn halt_forever() -> ! {
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nostack, nomem));
        }
    }
}
