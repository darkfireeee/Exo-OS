//! # syscall/fs_bridge/cwd.rs

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

pub const CWD_MAX_LEN: usize = 256;

pub const CWD_MAP_SIZE: usize = 512; // max PIDs simultanés dans la map

#[repr(C, align(64))]
pub struct CwdEntry {
    pub path: [u8; CWD_MAX_LEN],
    pub len: u32,
    pub pid: u32, // 0 = slot libre
}

impl CwdEntry {
    pub const fn empty() -> Self {
        Self {
            path: [0u8; CWD_MAX_LEN],
            len: 0,
            pid: 0,
        }
    }
}

pub struct CwdMap {
    pub slots: [CwdEntry; CWD_MAP_SIZE],
}

pub static CWD_LOCK: SpinMutex<()> = SpinMutex::new(());

/// Obtient l'index du slot CWD pour `pid`. Crée le slot si absent.
pub fn cwd_slot_for_pid(pid: u32) -> Option<usize> {
    // SAFETY: appelé sous CWD_LOCK.
    let map = unsafe { &mut *core::ptr::addr_of_mut!(CWD_MAP) };
    // Chercher slot existant
    for (i, slot) in map.slots.iter().enumerate() {
        if slot.pid == pid {
            return Some(i);
        }
    }
    // Allouer slot libre
    for (i, slot) in map.slots.iter_mut().enumerate() {
        if slot.pid == 0 {
            slot.pid = pid;
            // Initialiser à "/"
            slot.path[0] = b'/';
            slot.len = 1;
            return Some(i);
        }
    }
    None // table pleine
}

/// Libère le slot CWD d'un processus (appelé par do_exit).
pub fn cwd_release_pid(pid: u32) {
    let _guard = CWD_LOCK.lock();
    let map = unsafe { &mut *core::ptr::addr_of_mut!(CWD_MAP) };
    for slot in map.slots.iter_mut() {
        if slot.pid == pid {
            slot.pid = 0;
            slot.len = 0;
            slot.path[0] = 0;
            return;
        }
    }
}

#[inline]
pub fn process_umask(pid: u32) -> u32 {
    crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(pid))
        .map(|pcb| pcb.umask())
        .unwrap_or_else(|| POSIX_UMASK.load(Ordering::Acquire))
}

#[inline]
pub fn swap_process_umask(pid: u32, mask: u32) -> u32 {
    match crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(pid))
    {
        Some(pcb) => pcb.swap_umask(mask),
        None => POSIX_UMASK.swap(mask & 0o777, Ordering::AcqRel),
    }
}

#[inline]
pub fn apply_umask(mode: u32, default_perms: u32, pid: u32) -> u32 {
    let requested = if mode & 0o777 != 0 {
        mode & 0o777
    } else {
        default_perms
    };
    requested & !process_umask(pid) & 0o777
}

/// `getcwd(buf, size)`; ExoFS currently exposes a process-neutral root cwd.
#[inline]
pub fn fs_getcwd(buf_ptr: u64, size: usize, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if buf_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    if size < 2 {
        return Err(FsBridgeError::Invalid);
    }
    // FIX-FND-5: lire le chemin réel depuis CWD_MAP au lieu de retourner "/" fixe.
    let (path_bytes, path_len) = {
        let _guard = CWD_LOCK.lock();
        let map = unsafe { core::ptr::addr_of!(CWD_MAP).as_ref().unwrap_unchecked() };
        // Trouver le slot du PID courant
        let mut found_path = [0u8; CWD_MAX_LEN];
        let mut found_len = 1usize;
        found_path[0] = b'/'; // fallback
        for slot in map.slots.iter() {
            if slot.pid == pid && slot.len > 0 {
                let len = slot.len as usize;
                found_path[..len].copy_from_slice(&slot.path[..len]);
                found_len = len;
                break;
            }
        }
        (found_path, found_len)
    };
    // Copier vers l'espace utilisateur (path + null terminator)
    let copy_len = path_len.min(size.saturating_sub(1));
    copy_to_user(buf_ptr as *mut u8, path_bytes.as_ptr(), copy_len)
        .map_err(|_| FsBridgeError::Fault)?;
    // Écrire le null terminator
    copy_to_user(
        (buf_ptr + copy_len as u64) as *mut u8,
        &[0u8] as *const u8,
        1,
    )
    .map_err(|_| FsBridgeError::Fault)?;
    Ok((copy_len + 1) as i64)
}

/// `chdir(path)` — FIX-FND-5 : sauvegarde le CWD dans CWD_MAP.
#[inline]
pub fn fs_chdir(path: &[u8], pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    // 1. Valider le chemin (doit exister et être un répertoire)
    let normalized_path = resolve_path_with_symlinks(path, true, false)?;
    let (_, kind) = path_entry(&normalized_path)?;
    if kind != PATH_INDEX_KIND_DIR {
        return Err(FsBridgeError::NotDir);
    }
    // 2. FIX-FND-5 : sauvegarder le chemin normalisé dans CWD_MAP.
    // L'ancienne implémentation validait le chemin mais ne le stockait jamais,
    // laissant getcwd() toujours retourner "/" même après chdir("/tmp").
    // FIX: normalized_path est Vec<u8> — utiliser .as_slice() et non .as_bytes()
    let path_bytes: &[u8] = normalized_path.as_slice();
    if path_bytes.len() >= CWD_MAX_LEN {
        return Err(FsBridgeError::BadPath);
    }
    {
        let _guard = CWD_LOCK.lock();
        // SAFETY: accès exclusif garanti par CWD_LOCK.
        let map = unsafe { &mut *core::ptr::addr_of_mut!(CWD_MAP) };
        let idx = match cwd_slot_for_pid(pid) {
            Some(i) => i,
            None => return Err(FsBridgeError::NoMemory), // CWD_MAP pleine
        };
        let len = path_bytes.len();
        map.slots[idx].path[..len].copy_from_slice(path_bytes);
        map.slots[idx].len = len as u32;
        // Toujours null-terminer
        map.slots[idx].path[len] = 0;
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

