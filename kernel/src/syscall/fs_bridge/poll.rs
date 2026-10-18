//! # syscall/fs_bridge/poll.rs

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

#[inline]
pub fn set_fdset_bit(set: &mut [u8], fd: usize, present: bool) {
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
pub fn fdset_bit(set: &[u8], fd: usize) -> bool {
    let byte = fd / 8;
    let bit = fd % 8;
    byte < set.len() && (set[byte] & (1u8 << bit)) != 0
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

