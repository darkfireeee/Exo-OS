#![no_std]
#![no_main]

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
//! - VFS_RESOLVE  (2) : résoudre un chemin -> (blob_id, mount_id)
//! - VFS_OPEN     (3) : ouvrir un fichier -> fd dans le namespace appelant
//!
//! ## Syscalls utilisés
//! - SYS_EXOFS_PATH_RESOLVE = 500 (résolution chemin → blob_id)
//! - SYS_EXOFS_OBJECT_OPEN  = 501 (open blob → fd)
//! - SYS_IPC_REGISTER = 304, SYS_IPC_RECV = 301, SYS_IPC_SEND = 300

use exo_syscall_abi as syscall;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU32, Ordering};

// ── Table de montages ─────────────────────────────────────────────────────────

/// Types de pseudo-FS supportés.
#[repr(u8)]
#[derive(Copy, Clone, PartialEq)]
enum FsType {
    None    = 0,
    ExoFs   = 1,
    ProcFs  = 2,
    SysFs   = 3,
    DevFs   = 4,
}

/// Entrée de la table de montages.
#[derive(Copy, Clone)]
#[allow(dead_code)]
struct MountEntry {
    fs_type:    FsType,
    /// Hash FNV-32 du chemin de montage (ex : hash("/proc")).
    path_hash:  u32,
    /// BlobId du répertoire racine ExoFS (0 pour pseudo-FS).
    root_blob:  u64,
    active:     bool,
}

impl MountEntry {
    const fn empty() -> Self {
        Self { fs_type: FsType::None, path_hash: 0, root_blob: 0, active: false }
    }
}

static MOUNT_COUNT: AtomicU32 = AtomicU32::new(0);
static mut MOUNTS: [MountEntry; 32] = [MountEntry::empty(); 32];
static IPC_RECV_TIMEOUTS: AtomicU32 = AtomicU32::new(0);

fn fnv32(s: &[u8]) -> u32 {
    let mut h: u32 = 2166136261;
    for &b in s { h = h.wrapping_mul(16777619).wrapping_add(b as u32); }
    h
}

// ── Messages IPC ─────────────────────────────────────────────────────────────

const VFS_MOUNT:   u32 = 0;
const VFS_UMOUNT:  u32 = 1;
const VFS_RESOLVE: u32 = 2;
const VFS_OPEN:    u32 = 3;
const IPC_RECV_TIMEOUT_MS: u64 = 5_000;
const IPC_FLAG_TIMEOUT: u64 = syscall::IPC_FLAG_TIMEOUT;
const ETIMEDOUT: i64 = syscall::ETIMEDOUT;

#[repr(C)]
struct VfsRequest {
    sender_pid: u32,
    msg_type:   u32,
    payload:    [u8; 120],
}

#[repr(C)]
struct VfsReply {
    status:  i64,
    blob_id: u64,
    fd:      i64,
    _pad:    [u8; 40],
}

fn handle_mount(payload: &[u8]) -> VfsReply {
    // payload[0] = fstype u8, payload[1..5] = flags u32 LE,
    // payload[5..13] = root_blob u64 LE, payload[13..] = chemin null-terminated
    if payload.len() < 14 {
        return VfsReply { status: -22, blob_id: 0, fd: -1, _pad: [0; 40] }; // -EINVAL
    }
    let fstype = payload[0];
    let root_blob = u64::from_le_bytes([
        payload[5], payload[6], payload[7], payload[8],
        payload[9], payload[10], payload[11], payload[12],
    ]);
    let path = &payload[13..];
    // Trouver le null terminator
    let path_len = path.iter().position(|&b| b == 0).unwrap_or(path.len());
    let path_hash = fnv32(&path[..path_len]);

    let fs = match fstype {
        1 => FsType::ExoFs,
        2 => FsType::ProcFs,
        3 => FsType::SysFs,
        4 => FsType::DevFs,
        _ => return VfsReply { status: -22, blob_id: 0, fd: -1, _pad: [0; 40] },
    };

    let mut free_idx = None;
    unsafe {
        for i in 0..MOUNTS.len() {
            if MOUNTS[i].active && MOUNTS[i].path_hash == path_hash {
                MOUNTS[i] = MountEntry { fs_type: fs, path_hash, root_blob, active: true };
                return VfsReply { status: 0, blob_id: root_blob, fd: i as i64, _pad: [0; 40] };
            }
            if free_idx.is_none() && !MOUNTS[i].active {
                free_idx = Some(i);
            }
        }
    }

    let idx = match free_idx {
        Some(i) => i,
        None => return VfsReply { status: -28, blob_id: 0, fd: -1, _pad: [0; 40] },
    };

    unsafe {
        MOUNTS[idx] = MountEntry { fs_type: fs, path_hash, root_blob, active: true };
    }
    MOUNT_COUNT.fetch_add(1, Ordering::AcqRel);

    VfsReply { status: 0, blob_id: root_blob, fd: idx as i64, _pad: [0; 40] }
}

fn handle_resolve(payload: &[u8]) -> VfsReply {
    // payload = chemin null-terminated
    let path_len = payload.iter().position(|&b| b == 0).unwrap_or(payload.len());
    let path = &payload[..path_len];

    // Appel SYS_EXOFS_PATH_RESOLVE(path_ptr, path_len) → blob_id i64
    let blob_id = unsafe {
        syscall::syscall2(
            syscall::SYS_EXOFS_PATH_RESOLVE,
            path.as_ptr() as u64,
            path_len as u64,
        )
    };

    if blob_id < 0 {
        VfsReply { status: blob_id, blob_id: 0, fd: -1, _pad: [0; 40] }
    } else {
        VfsReply { status: 0, blob_id: blob_id as u64, fd: -1, _pad: [0; 40] }
    }
}

fn handle_open(payload: &[u8]) -> VfsReply {
    // payload[0..8] = blob_id u64, payload[8..12] = flags u32
    if payload.len() < 12 {
        return VfsReply { status: -22, blob_id: 0, fd: -1, _pad: [0; 40] };
    }
    let blob_id = u64::from_le_bytes([
        payload[0], payload[1], payload[2], payload[3],
        payload[4], payload[5], payload[6], payload[7],
    ]);
    let _flags = u32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]);

    let fd = unsafe {
        syscall::syscall3(
            syscall::SYS_EXOFS_OBJECT_OPEN,
            blob_id,
            syscall::O_RDONLY,
            0,
        )
    };

    if fd < 0 {
        VfsReply { status: fd, blob_id: blob_id, fd: -1, _pad: [0; 40] }
    } else {
        VfsReply { status: 0, blob_id: blob_id, fd, _pad: [0; 40] }
    }
}

fn handle_umount(payload: &[u8]) -> VfsReply {
    let path_len = payload.iter().position(|&b| b == 0).unwrap_or(payload.len());
    if path_len == 0 {
        return VfsReply { status: -22, blob_id: 0, fd: -1, _pad: [0; 40] };
    }

    let path_hash = fnv32(&payload[..path_len]);
    unsafe {
        for i in 0..MOUNTS.len() {
            if MOUNTS[i].active && MOUNTS[i].path_hash == path_hash {
                MOUNTS[i] = MountEntry::empty();
                MOUNT_COUNT.fetch_update(Ordering::AcqRel, Ordering::Relaxed, |count| {
                    Some(count.saturating_sub(1))
                }).ok();
                return VfsReply { status: 0, blob_id: 0, fd: i as i64, _pad: [0; 40] };
            }
        }
    }

    VfsReply { status: -2, blob_id: 0, fd: -1, _pad: [0; 40] }
}

fn handle_request(req: &VfsRequest) -> VfsReply {
    match req.msg_type {
        VFS_MOUNT   => handle_mount(&req.payload),
        VFS_RESOLVE => handle_resolve(&req.payload),
        VFS_OPEN    => handle_open(&req.payload),
        VFS_UMOUNT  => handle_umount(&req.payload),
        _ => VfsReply { status: -22, blob_id: 0, fd: -1, _pad: [0; 40] },
    }
}

/// Monte les pseudo-filesystems de base au démarrage.
fn mount_default_namespaces() {
    // /proc (ProcFs, pas de blob racine)
    let proc_entry = MountEntry {
        fs_type: FsType::ProcFs, path_hash: fnv32(b"/proc"), root_blob: 0, active: true,
    };
    // /sys (SysFs)
    let sys_entry = MountEntry {
        fs_type: FsType::SysFs,  path_hash: fnv32(b"/sys"),  root_blob: 0, active: true,
    };
    // /dev (DevFs)
    let dev_entry = MountEntry {
        fs_type: FsType::DevFs,  path_hash: fnv32(b"/dev"),  root_blob: 0, active: true,
    };

    unsafe {
        MOUNTS[0] = proc_entry;
        MOUNTS[1] = sys_entry;
        MOUNTS[2] = dev_entry;
    }
    MOUNT_COUNT.store(3, Ordering::Release);
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
    let mut req = VfsRequest { sender_pid: 0, msg_type: 0, payload: [0u8; 120] };

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
        if r < 0 { continue; }

        let reply = handle_request(&req);

        let _ = unsafe {
            syscall::syscall6(
                syscall::SYS_IPC_SEND,
                req.sender_pid as u64,
                &reply as *const VfsReply as u64,
                core::mem::size_of::<VfsReply>() as u64,
                0, 0, 0,
            )
        };
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop { unsafe { core::arch::asm!("hlt", options(nostack, nomem)); } }
}
