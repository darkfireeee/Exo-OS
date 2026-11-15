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

#[derive(Clone, Copy)]
struct EpollWatch {
    pid: u32,
    ep_blob: BlobId,
    fd: u32,
    events: u32,
    data: u64,
}

static EPOLL_WATCHES: Mutex<Vec<EpollWatch>> = Mutex::new(Vec::new());

pub fn remove_epoll_watches_for_fd(pid: u32, fd: u32) {
    let mut watches = EPOLL_WATCHES.lock();
    let mut i = 0;
    while i < watches.len() {
        if watches[i].pid == pid && watches[i].fd == fd {
            watches.swap_remove(i);
        } else {
            i += 1;
        }
    }
}

pub fn remove_epoll_watches_for_blob(ep_blob: BlobId) {
    let mut watches = EPOLL_WATCHES.lock();
    let mut i = 0;
    while i < watches.len() {
        if watches[i].ep_blob == ep_blob {
            watches.swap_remove(i);
        } else {
            i += 1;
        }
    }
}

pub fn epoll_has_ready_watch(ep_blob: BlobId, pid: u32) -> bool {
    let snapshot: Vec<EpollWatch> = {
        let watches = EPOLL_WATCHES.lock();
        let mut out = Vec::new();
        for w in watches.iter() {
            if w.pid == pid && w.ep_blob == ep_blob && out.try_reserve(1).is_ok() {
                out.push(*w);
            }
        }
        out
    };
    for w in snapshot {
        if let Ok((readable, writable)) = fd_readiness(w.fd, pid) {
            if (readable && (w.events & 0x001) != 0) || (writable && (w.events & 0x004) != 0) {
                return true;
            }
            if let Ok(res) = resolve_fd(pid, w.fd) {
                if !is_tty_handle(res.handle) {
                    if let Ok(entry) = OBJECT_TABLE.get(res.handle) {
                        if is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG)
                            && is_socket_peer_closed(entry.blob_id)
                        {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
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
    let target_entry = OBJECT_TABLE
        .get(target_obj)
        .map_err(exofs_to_bridge_error)?;
    if fd == epfd
        || target_entry.blob_id == ep_entry.blob_id
        || is_pseudo_blob(&target_entry.blob_id, PSEUDO_EPOLL_TAG)
    {
        return Err(FsBridgeError::Invalid);
    }
    let parsed_ev = if op == EPOLL_CTL_ADD || op == EPOLL_CTL_MOD {
        if event_ptr == 0 {
            return Err(FsBridgeError::Fault);
        }
        Some(read_user_typed::<LinuxEpollEvent>(event_ptr).map_err(|_| FsBridgeError::Fault)?)
    } else {
        None
    };
    let mut watches = EPOLL_WATCHES.lock();
    let existing_pos = watches
        .iter()
        .position(|w| w.pid == pid && w.ep_blob == ep_entry.blob_id && w.fd == fd);
    match op {
        EPOLL_CTL_ADD => {
            if existing_pos.is_some() {
                return Err(FsBridgeError::Exists);
            }
            let ev = parsed_ev.ok_or(FsBridgeError::Invalid)?;
            watches
                .try_reserve(1)
                .map_err(|_| FsBridgeError::NoMemory)?;
            watches.push(EpollWatch {
                pid,
                ep_blob: ep_entry.blob_id,
                fd,
                events: ev.events,
                data: ev.data,
            });
        }
        EPOLL_CTL_MOD => {
            let pos = existing_pos.ok_or(FsBridgeError::NotFound)?;
            let ev = parsed_ev.ok_or(FsBridgeError::Invalid)?;
            watches[pos].events = ev.events;
            watches[pos].data = ev.data;
        }
        EPOLL_CTL_DEL => {
            let pos = existing_pos.ok_or(FsBridgeError::NotFound)?;
            watches.swap_remove(pos);
        }
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

    let start_ns = crate::scheduler::timer::clock::monotonic_ns();
    let timeout_ns = if timeout > 0 {
        (timeout as u64).saturating_mul(1_000_000)
    } else {
        0
    };

    loop {
        let snapshot: Vec<EpollWatch> = {
            let watches = EPOLL_WATCHES.lock();
            let mut out = Vec::new();
            for w in watches.iter() {
                if w.pid == pid && w.ep_blob == ep_entry.blob_id {
                    let _ = out.try_reserve(1);
                    out.push(*w);
                }
            }
            out
        };

        let mut ready = 0usize;
        let max_out = (maxevents as usize).min(1024);
        for w in snapshot {
            if ready >= max_out {
                break;
            }
            match fd_readiness(w.fd, pid) {
                Ok((readable, writable)) => {
                    let mut revents = 0u32;
                    if readable && (w.events & 0x001) != 0 {
                        revents |= 0x001;
                    }
                    if writable && (w.events & 0x004) != 0 {
                        revents |= 0x004;
                    }
                    if let Ok(res) = resolve_fd(pid, w.fd) {
                        if !is_tty_handle(res.handle) {
                            if let Ok(entry) = OBJECT_TABLE.get(res.handle) {
                                if is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG)
                                    && is_socket_peer_closed(entry.blob_id)
                                {
                                    revents |= 0x010; // EPOLLHUP
                                }
                            }
                        }
                    }
                    if revents != 0 {
                        let addr = events_ptr
                            .checked_add(
                                (ready as u64).saturating_mul(size_of::<LinuxEpollEvent>() as u64),
                            )
                            .ok_or(FsBridgeError::Fault)?;
                        write_user_typed(
                            addr,
                            LinuxEpollEvent {
                                events: revents,
                                data: w.data,
                            },
                        )
                        .map_err(|_| FsBridgeError::Fault)?;
                        ready += 1;
                    }
                }
                Err(_) => {
                    let mut watches = EPOLL_WATCHES.lock();
                    if let Some(pos) = watches
                        .iter()
                        .position(|x| x.pid == pid && x.ep_blob == ep_entry.blob_id && x.fd == w.fd)
                    {
                        watches.swap_remove(pos);
                    }
                }
            }
        }

        if ready > 0 || timeout == 0 {
            return Ok(ready as i64);
        }
        if timeout > 0 {
            let elapsed = crate::scheduler::timer::clock::monotonic_ns().saturating_sub(start_ns);
            if elapsed >= timeout_ns {
                return Ok(0);
            }
        }
        if !crate::scheduler::timer::sleep::sleep_ns(1_000_000) {
            return Err(FsBridgeError::Interrupted);
        }
    }
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
    if nfds > 1024 {
        return Err(FsBridgeError::Invalid);
    }
    if nfds == 0 {
        if timeout == 0 {
            return Ok(0);
        }
        if timeout > 0 {
            let wait_ns = (timeout as u64).saturating_mul(1_000_000);
            if !crate::scheduler::timer::sleep::sleep_ns(wait_ns) {
                return Err(FsBridgeError::Interrupted);
            }
            return Ok(0);
        }
        loop {
            if !crate::scheduler::timer::sleep::sleep_ns(100_000_000) {
                return Err(FsBridgeError::Interrupted);
            }
        }
    }
    if fds_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }

    let start_ns = crate::scheduler::timer::clock::monotonic_ns();
    let timeout_ns = if timeout > 0 {
        (timeout as u64).saturating_mul(1_000_000)
    } else {
        0
    };

    loop {
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
                        if let Ok(res) = resolve_fd(pid, pfd.fd as u32) {
                            if !is_tty_handle(res.handle) {
                                if let Ok(entry) = OBJECT_TABLE.get(res.handle) {
                                    if is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG)
                                        && is_socket_peer_closed(entry.blob_id)
                                    {
                                        pfd.revents |= POLLHUP;
                                    }
                                }
                            }
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
        if ready > 0 || timeout == 0 {
            return Ok(ready);
        }
        if timeout > 0 {
            let elapsed = crate::scheduler::timer::clock::monotonic_ns().saturating_sub(start_ns);
            if elapsed >= timeout_ns {
                return Ok(0);
            }
        }
        if !crate::scheduler::timer::sleep::sleep_ns(1_000_000) {
            return Err(FsBridgeError::Interrupted);
        }
    }
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
    if nfds > 1024 {
        return Err(FsBridgeError::Invalid);
    }
    let timeout_ns: Option<u64> = if timeout_ptr == 0 {
        None
    } else {
        let tv = read_user_typed::<LinuxTimeval>(timeout_ptr).map_err(|_| FsBridgeError::Fault)?;
        if tv.tv_sec < 0 || tv.tv_usec < 0 || tv.tv_usec >= 1_000_000 {
            return Err(FsBridgeError::Invalid);
        }
        Some(
            (tv.tv_sec as u64)
                .saturating_mul(1_000_000_000)
                .saturating_add((tv.tv_usec as u64).saturating_mul(1_000)),
        )
    };
    let bytes = nfds.saturating_add(7) / 8;
    let mut orig_read = vec_with_zeroes(bytes)?;
    let mut orig_write = vec_with_zeroes(bytes)?;
    let mut orig_except = vec_with_zeroes(bytes)?;

    if readfds_ptr != 0 && bytes != 0 {
        copy_from_user(orig_read.as_mut_ptr(), readfds_ptr as *const u8, bytes)
            .map_err(|_| FsBridgeError::Fault)?;
    }
    if writefds_ptr != 0 && bytes != 0 {
        copy_from_user(orig_write.as_mut_ptr(), writefds_ptr as *const u8, bytes)
            .map_err(|_| FsBridgeError::Fault)?;
    }
    if exceptfds_ptr != 0 && bytes != 0 {
        copy_from_user(orig_except.as_mut_ptr(), exceptfds_ptr as *const u8, bytes)
            .map_err(|_| FsBridgeError::Fault)?;
    }

    let start_ns = crate::scheduler::timer::clock::monotonic_ns();
    let mut out_read = vec_with_zeroes(bytes)?;
    let mut out_write = vec_with_zeroes(bytes)?;
    let out_except = vec_with_zeroes(bytes)?;

    let ready = loop {
        out_read.fill(0);
        out_write.fill(0);
        let mut ready = 0i64;
        let mut fd = 0usize;
        while fd < nfds {
            let check_r = readfds_ptr != 0 && fdset_bit(&orig_read, fd);
            let check_w = writefds_ptr != 0 && fdset_bit(&orig_write, fd);
            if check_r || check_w {
                let (readable, writable) = fd_readiness(fd as u32, pid).unwrap_or((false, false));
                if check_r && readable {
                    set_fdset_bit(&mut out_read, fd, true);
                    ready += 1;
                }
                if check_w && writable {
                    set_fdset_bit(&mut out_write, fd, true);
                    ready += 1;
                }
            }
            fd += 1;
        }
        if ready > 0 || timeout_ns == Some(0) {
            break ready;
        }
        if let Some(limit) = timeout_ns {
            let elapsed = crate::scheduler::timer::clock::monotonic_ns().saturating_sub(start_ns);
            if elapsed >= limit {
                break 0;
            }
            let remaining = limit - elapsed;
            if !crate::scheduler::timer::sleep::sleep_ns(remaining.min(1_000_000)) {
                return Err(FsBridgeError::Interrupted);
            }
        } else if !crate::scheduler::timer::sleep::sleep_ns(1_000_000) {
            return Err(FsBridgeError::Interrupted);
        }
    };

    if readfds_ptr != 0 && bytes != 0 {
        copy_to_user(readfds_ptr as *mut u8, out_read.as_ptr(), bytes)
            .map_err(|_| FsBridgeError::Fault)?;
    }
    if writefds_ptr != 0 && bytes != 0 {
        copy_to_user(writefds_ptr as *mut u8, out_write.as_ptr(), bytes)
            .map_err(|_| FsBridgeError::Fault)?;
    }
    if exceptfds_ptr != 0 && bytes != 0 {
        copy_to_user(exceptfds_ptr as *mut u8, out_except.as_ptr(), bytes)
            .map_err(|_| FsBridgeError::Fault)?;
    }
    Ok(ready)
}

