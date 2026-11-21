//! # syscall/fs_bridge/fd.rs

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

#[derive(Clone, Copy)]
pub struct ResolvedFd {
    pub handle: u32,
    pub flags: u32,
}

#[inline]
pub fn fd_can_read_flags(flags: u32) -> bool {
    let rw = flags & 0x3;
    rw == open_flags::O_RDONLY || rw == open_flags::O_RDWR
}

#[inline]
pub fn fd_can_write_flags(flags: u32) -> bool {
    let rw = flags & 0x3;
    rw == open_flags::O_WRONLY || rw == open_flags::O_RDWR
}

#[inline]
pub fn fd_table_flags(open_flags_raw: u32, fd_flags: u32) -> u32 {
    (fd_flags & 0x3)
        | (fd_flags & open_flags::O_APPEND)
        | (open_flags_raw & (O_CLOEXEC | O_NONBLOCK))
}

#[inline]
pub fn process_has_fd_table(pid: u32) -> bool {
    crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(pid))
        .is_some()
}

#[inline]
pub fn process_fd_descriptor(pid: u32, fd: u32) -> Option<crate::process::core::pcb::FileDescriptor> {
    crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(pid))
        .and_then(|pcb| pcb.files.lock().get(fd as i32).copied())
}

#[inline]
pub fn resolve_fd(pid: u32, fd: u32) -> Result<ResolvedFd, FsBridgeError> {
    if let Some(desc) = process_fd_descriptor(pid, fd) {
        if desc.handle > u32::MAX as u64 {
            return Err(FsBridgeError::BadFd);
        }
        return Ok(ResolvedFd {
            handle: desc.handle as u32,
            flags: desc.flags,
        });
    }

    // If the process has a descriptor table, an absent fd is strictly invalid (T9-001 FIX).
    if process_has_fd_table(pid) || fd <= 2 {
        return Err(FsBridgeError::BadFd);
    }

    #[cfg(test)]
    {
        Ok(ResolvedFd {
            handle: fd,
            flags: open_flags::O_RDWR,
        })
    }

    #[cfg(not(test))]
    Err(FsBridgeError::BadFd)
}

#[inline]
pub fn install_process_fd(pid: u32, handle: u64, flags: u32) -> Option<i32> {
    crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(pid))
        .and_then(|pcb| {
            let mut files = pcb.files.lock();
            let fd = files.install(handle, flags);
            (fd >= 0).then_some(fd)
        })
}

#[inline]
pub fn install_process_fd_at(pid: u32, fd: u32, handle: u64, flags: u32) -> bool {
    crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(pid))
        .map(|pcb| pcb.files.lock().install_at(fd as i32, handle, flags))
        .unwrap_or(false)
}

#[inline]
pub fn close_process_fd(pid: u32, fd: u32) -> Option<u64> {
    crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(pid))
        .and_then(|pcb| pcb.files.lock().close(fd as i32))
}

#[inline]
pub fn set_process_fd_flags(pid: u32, fd: u32, flags: u32) -> bool {
    crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(pid))
        .map(|pcb| pcb.files.lock().set_flags(fd as i32, flags))
        .unwrap_or(false)
}

#[inline]
pub fn fd_readiness_mask(fd: u32, pid: u32) -> Result<u32, FsBridgeError> {
    let resolved = resolve_fd(pid, fd)?;
    if is_tty_handle(resolved.handle) {
        let mut mask = 0u32;
        if fd_can_read_flags(resolved.flags) {
            mask |= EPOLLIN;
        }
        if fd_can_write_flags(resolved.flags) {
            mask |= EPOLLOUT;
        }
        return Ok(mask);
    }
    let entry = OBJECT_TABLE
        .get(resolved.handle)
        .map_err(exofs_to_bridge_error)?;

    if is_pseudo_blob(&entry.blob_id, PSEUDO_PIPE_TAG) {
        let mut mask = 0u32;
        let len = blob_len(&entry.blob_id);
        let open_cnt = OBJECT_TABLE.open_count_for(&entry.blob_id);
        let read_cnt = OBJECT_TABLE.readable_count_for(&entry.blob_id);
        let no_writers = open_cnt <= read_cnt;
        if entry.can_read() {
            if len != 0 || no_writers {
                mask |= EPOLLIN;
            }
            if no_writers {
                mask |= EPOLLHUP;
            }
        }
        if entry.can_write() {
            if read_cnt == 0 {
                mask |= EPOLLERR;
            } else if len < PIPE_MAX_CAPACITY {
                mask |= EPOLLOUT;
            }
        }
        return Ok(mask);
    }

    if is_pseudo_blob(&entry.blob_id, PSEUDO_EVENTFD_TAG) {
        let mut mask = 0u32;
        if let Ok((value, _)) = eventfd_state(entry.blob_id) {
            if entry.can_read() && value != 0 {
                mask |= EPOLLIN;
            }
            if entry.can_write() && value < u64::MAX - 1 {
                mask |= EPOLLOUT;
            }
        }
        return Ok(mask);
    }

    if is_pseudo_blob(&entry.blob_id, PSEUDO_INOTIFY_TAG) {
        return Ok(0);
    }

    if is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG) {
        return Ok(socket_readiness_mask(entry.blob_id));
    }

    if is_pseudo_blob(&entry.blob_id, PSEUDO_NET_SOCKET_TAG) {
        return Ok(crate::syscall::net_bridge::net_poll_mask_for_blob(entry.blob_id));
    }

    if is_pseudo_blob(&entry.blob_id, PSEUDO_EPOLL_TAG) {
        let mut mask = 0u32;
        if entry.can_read() && epoll_has_ready_watch(entry.blob_id, pid) {
            mask |= EPOLLIN;
        }
        return Ok(mask);
    }

    let mut mask = 0u32;
    if entry.can_read() {
        mask |= EPOLLIN;
    }
    if entry.can_write() {
        mask |= EPOLLOUT;
    }
    Ok(mask)
}

#[inline]
pub fn fd_readiness(fd: u32, pid: u32) -> Result<(bool, bool), FsBridgeError> {
    let mask = fd_readiness_mask(fd, pid)?;
    Ok(((mask & EPOLLIN) != 0, (mask & EPOLLOUT) != 0))
}

#[inline]
pub fn flock_range_from_abi(fl: LinuxFlock) -> Result<(u64, u64), FsBridgeError> {
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

static LOCK_WAIT_GEN: AtomicU64 = AtomicU64::new(0);
static LOCK_WAITERS: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// Wakes all threads blocked on `F_SETLKW` or blocking `flock`.
pub fn notify_lock_waiters() {
    LOCK_WAIT_GEN.fetch_add(1, Ordering::AcqRel);
    let to_wake: Vec<u64> = {
        let mut waiters = LOCK_WAITERS.lock();
        let mut out = Vec::new();
        core::mem::swap(&mut out, &mut *waiters);
        out
    };
    for tcb_raw in to_wake {
        crate::scheduler::timer::sleep::wake_sleeping_tcb(tcb_raw);
    }
}

fn wait_for_lock_event(observed_gen: u64) -> Result<(), FsBridgeError> {
    let tcb = crate::scheduler::core::switch::current_thread_raw() as u64;
    if tcb == 0 {
        return Err(FsBridgeError::WouldBlock);
    }
    {
        let mut waiters = LOCK_WAITERS.lock();
        if LOCK_WAIT_GEN.load(Ordering::Acquire) != observed_gen {
            return Ok(());
        }
        if !waiters.contains(&tcb) {
            let _ = waiters.try_reserve(1);
            if waiters.capacity() > waiters.len() {
                waiters.push(tcb);
            }
        }
    }
    let ok = crate::scheduler::timer::sleep::sleep_ns_if(100_000_000, || {
        LOCK_WAIT_GEN.load(Ordering::Acquire) == observed_gen
    });
    {
        let mut waiters = LOCK_WAITERS.lock();
        if let Some(pos) = waiters.iter().position(|&w| w == tcb) {
            waiters.swap_remove(pos);
        }
    }
    if !ok {
        return Err(FsBridgeError::Interrupted);
    }
    Ok(())
}

/// Close an `OBJECT_TABLE` handle and clean up any pseudo-blob when its last reference drops.
#[inline]
pub fn close_object_handle_with_cleanup(obj_fd: u32) -> bool {
    if let Some(blob_id) = OBJECT_TABLE.close_for_cleanup(obj_fd) {
        if OBJECT_TABLE.open_count_for(&blob_id) == 0 {
            cleanup_closed_blob(blob_id);
        }
        notify_io_event(Some(blob_id));
        true
    } else {
        false
    }
}

/// `close(fd)`.
#[inline]
pub fn fs_close(fd: u32, pid: u32) -> Result<i64, FsBridgeError> {
    let resolved = resolve_fd(pid, fd)?;
    let removed_from_pcb = close_process_fd(pid, fd);
    let handle = removed_from_pcb.unwrap_or(resolved.handle as u64);
    remove_epoll_watches_for_fd(pid, fd);
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
    if removed_from_pcb.is_none() && pid != 0 && entry.owner_uid != 0 && entry.owner_uid != pid as u64 {
        return Err(FsBridgeError::BadFd);
    }
    let closed_blob = entry.blob_id;
    if close_object_handle_with_cleanup(obj_fd) {
        // POSIX: closing any fd referencing an inode releases the process's fcntl/flock locks on it.
        let object_id = inode_from_blob_id(&closed_blob);
        let _ = crate::fs::exofs::posix_bridge::fcntl_lock::FCNTL_LOCK_TABLE.release(
            object_id,
            pid as u64,
            0,
            0,
            u64::MAX,
        );
        notify_lock_waiters();
        notify_io_event(Some(closed_blob));
        Ok(0)
    } else {
        Err(FsBridgeError::BadFd)
    }
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
        let _ = close_object_handle_with_cleanup(handle);
        Err(FsBridgeError::NoMemory)
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
    remove_epoll_watches_for_fd(pid, newfd);
    if let Some(old_handle) = close_process_fd(pid, newfd) {
        if !is_tty_handle_u64(old_handle) && old_handle <= u32::MAX as u64 {
            let _ = close_object_handle_with_cleanup(old_handle as u32);
        }
    }
    // POSIX specifies that FD_CLOEXEC must always be cleared on newfd during dup2
    let dup_flags = resolved.flags & !O_CLOEXEC;
    if is_tty_handle(resolved.handle) {
        if install_process_fd_at(pid, newfd, TTY_PTS0_HANDLE as u64, dup_flags) {
            return Ok(newfd as i64);
        }
        return Err(FsBridgeError::NoMemory);
    }
    if process_has_fd_table(pid) {
        let handle = OBJECT_TABLE
            .dup(resolved.handle)
            .map_err(exofs_to_bridge_error)?;
        if install_process_fd_at(pid, newfd, handle as u64, dup_flags) {
            Ok(newfd as i64)
        } else {
            let _ = close_object_handle_with_cleanup(handle);
            Err(FsBridgeError::NoMemory)
        }
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
        F_DUPFD | F_DUPFD_CLOEXEC => {
            let resolved = resolve_fd(pid, fd)?;
            if arg > i32::MAX as u64 {
                return Err(FsBridgeError::Invalid);
            }
            let min_fd = arg as i32;
            let target_flags = if cmd == F_DUPFD_CLOEXEC {
                resolved.flags | O_CLOEXEC
            } else {
                resolved.flags & !O_CLOEXEC
            };
            if is_tty_handle(resolved.handle) {
                if let Some(pcb) = crate::process::core::registry::PROCESS_REGISTRY
                    .find_by_pid(crate::process::core::pid::Pid(pid))
                {
                    let mut files = pcb.files.lock();
                    let mut candidate = min_fd.max(0);
                    while candidate < 65536 {
                        if files.get(candidate).is_none() {
                            if files.install_at(candidate, TTY_PTS0_HANDLE as u64, target_flags) {
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
                        if files.install_at(candidate, handle as u64, target_flags) {
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
            let _ = resolve_fd(pid, fd)?;
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
            // POSIX: F_SETFL ignores file access mode (O_ACCMODE) and file creation flags,
            // updating only mutable file status flags (O_APPEND | O_NONBLOCK).
            let resolved = resolve_fd(pid, fd)?;
            let mutable = (arg as u32) & (open_flags::O_APPEND | O_NONBLOCK);
            if is_tty_handle(resolved.handle) {
                let access = resolved.flags & (0x3 | O_CLOEXEC);
                let flags = access | (mutable & O_NONBLOCK);
                let _ = set_process_fd_flags(pid, fd, flags);
                Ok(0)
            } else {
                let flags = OBJECT_TABLE
                    .set_status_flags(resolved.handle, mutable)
                    .map_err(exofs_to_bridge_error)?;
                let descriptor_flags = (resolved.flags & (0x3 | O_CLOEXEC))
                    | (flags & (open_flags::O_APPEND | O_NONBLOCK));
                let _ = set_process_fd_flags(pid, fd, descriptor_flags);
                Ok(0)
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
            loop {
                let gen = LOCK_WAIT_GEN.load(Ordering::Acquire);
                match FCNTL_LOCK_TABLE.acquire(lock) {
                    Ok(()) => {
                        if kind == LockKind::Unlock {
                            notify_lock_waiters();
                        }
                        return Ok(0);
                    }
                    Err(ExofsError::PermissionDenied) => {
                        if cmd == F_SETLKW && kind != LockKind::Unlock {
                            wait_for_lock_event(gen)?;
                            continue;
                        }
                        return Err(FsBridgeError::WouldBlock);
                    }
                    Err(err) => return Err(exofs_to_bridge_error(err)),
                }
            }
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
    let nonblock = (operation & LOCK_NB) != 0;
    use crate::fs::exofs::posix_bridge::fcntl_lock::{
        make_lock, validate_lock, LockKind, FCNTL_LOCK_TABLE,
    };
    let lock = make_lock(object_id, pid as u64, 0, 0, u64::MAX, kind);
    if kind != LockKind::Unlock {
        validate_lock(&lock).map_err(exofs_to_bridge_error)?;
    }
    loop {
        let gen = LOCK_WAIT_GEN.load(Ordering::Acquire);
        match FCNTL_LOCK_TABLE.acquire(lock) {
            Ok(()) => {
                if kind == LockKind::Unlock {
                    notify_lock_waiters();
                }
                return Ok(0);
            }
            Err(ExofsError::PermissionDenied) => {
                if !nonblock && kind != LockKind::Unlock {
                    wait_for_lock_event(gen)?;
                    continue;
                }
                return Err(FsBridgeError::WouldBlock);
            }
            Err(err) => return Err(exofs_to_bridge_error(err)),
        }
    }
}

