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

// ─────────────────────────────────────────────────────────────────────────────
// Event-driven I/O wait hub & per-blob edge generation tracker
// ─────────────────────────────────────────────────────────────────────────────

static IO_WAIT_GEN: AtomicU64 = AtomicU64::new(0);
static IO_WAITERS: Mutex<Vec<u64>> = Mutex::new(Vec::new());
static BLOB_EDGE_SEQ: Mutex<Vec<(BlobId, u64)>> = Mutex::new(Vec::new());

/// Monotonic generation counter incremented on every kernel I/O readiness transition.
#[inline]
pub fn io_wait_gen() -> u64 {
    IO_WAIT_GEN.load(Ordering::Acquire)
}

/// Returns the current edge generation for `blob_id` (used by `EPOLLET`).
pub fn blob_edge_gen(blob_id: BlobId) -> u64 {
    BLOB_EDGE_SEQ
        .lock()
        .iter()
        .find(|(b, _)| *b == blob_id)
        .map(|(_, seq)| *seq)
        .unwrap_or(0)
}

/// Removes edge-tracking state when a blob is fully closed and reclaimed.
pub fn remove_blob_edge(blob_id: &BlobId) {
    let mut seqs = BLOB_EDGE_SEQ.lock();
    if let Some(pos) = seqs.iter().position(|(b, _)| b == blob_id) {
        seqs.swap_remove(pos);
    }
}

/// Notifies all threads sleeping on `epoll_wait`, `poll`, `select`, pipes, or sockets.
/// If `blob_id` is provided, also advances its per-blob edge sequence for `EPOLLET`.
pub fn notify_io_event(blob_id: Option<BlobId>) {
    if let Some(b) = blob_id {
        let mut seqs = BLOB_EDGE_SEQ.lock();
        if let Some((_, seq)) = seqs.iter_mut().find(|(id, _)| *id == b) {
            *seq = seq.wrapping_add(1).max(1);
        } else if seqs.len() < MAX_BLOB_EDGE_ENTRIES && seqs.try_reserve(1).is_ok() {
            seqs.push((b, 1));
        }
    }
    IO_WAIT_GEN.fetch_add(1, Ordering::AcqRel);
    let to_wake: Vec<u64> = {
        let mut waiters = IO_WAITERS.lock();
        let mut out = Vec::new();
        core::mem::swap(&mut out, &mut *waiters);
        out
    };
    for tcb_raw in to_wake {
        crate::scheduler::timer::sleep::wake_sleeping_tcb(tcb_raw);
    }
}

/// Event-driven wait on `IO_WAIT_HUB` until `IO_WAIT_GEN` changes from `observed_gen`,
/// `deadline_ns` expires, or a signal interrupts the thread (`FsBridgeError::Interrupted`).
///
/// Returns `Ok(true)` if woken or before deadline, `Ok(false)` if `deadline_ns` has elapsed.
pub fn wait_for_io_event(
    observed_gen: u64,
    deadline_ns: Option<u64>,
    max_slice_ns: u64,
) -> Result<bool, FsBridgeError> {
    if let Some(dl) = deadline_ns {
        if crate::scheduler::timer::clock::monotonic_ns() >= dl {
            return Ok(false);
        }
    }
    let tcb = crate::scheduler::core::switch::current_thread_raw() as u64;
    if tcb == 0 {
        let slice = match deadline_ns {
            Some(dl) => {
                let now = crate::scheduler::timer::clock::monotonic_ns();
                if now >= dl {
                    return Ok(false);
                }
                (dl - now).min(max_slice_ns.min(1_000_000))
            }
            None => max_slice_ns.min(1_000_000),
        };
        if !crate::scheduler::timer::sleep::sleep_ns(slice) {
            return Err(FsBridgeError::Interrupted);
        }
        return Ok(deadline_ns
            .map(|dl| crate::scheduler::timer::clock::monotonic_ns() < dl)
            .unwrap_or(true));
    }

    {
        let mut waiters = IO_WAITERS.lock();
        if IO_WAIT_GEN.load(Ordering::Acquire) != observed_gen {
            return Ok(true);
        }
        if !waiters.contains(&tcb) {
            if waiters.try_reserve(1).is_ok() {
                waiters.push(tcb);
            }
        }
    }

    let slice = match deadline_ns {
        Some(dl) => {
            let now = crate::scheduler::timer::clock::monotonic_ns();
            if now >= dl {
                let mut waiters = IO_WAITERS.lock();
                if let Some(pos) = waiters.iter().position(|&w| w == tcb) {
                    waiters.swap_remove(pos);
                }
                return Ok(false);
            }
            (dl - now).min(max_slice_ns)
        }
        None => max_slice_ns,
    };

    let ok = crate::scheduler::timer::sleep::sleep_ns_if(slice, || {
        IO_WAIT_GEN.load(Ordering::Acquire) == observed_gen
    });

    {
        let mut waiters = IO_WAITERS.lock();
        if let Some(pos) = waiters.iter().position(|&w| w == tcb) {
            waiters.swap_remove(pos);
        }
    }

    if !ok {
        return Err(FsBridgeError::Interrupted);
    }

    Ok(deadline_ns
        .map(|dl| crate::scheduler::timer::clock::monotonic_ns() < dl)
        .unwrap_or(true))
}

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

fn fd_needs_external_poll_slice(fd: u32, pid: u32) -> bool {
    let Ok(res) = resolve_fd(pid, fd) else {
        return false;
    };
    if is_tty_handle(res.handle) {
        return true;
    }
    OBJECT_TABLE
        .get(res.handle)
        .map(|entry| is_pseudo_blob(&entry.blob_id, PSEUDO_NET_SOCKET_TAG))
        .unwrap_or(false)
}

fn fd_blob_id(fd: u32, pid: u32) -> Option<BlobId> {
    let res = resolve_fd(pid, fd).ok()?;
    if is_tty_handle(res.handle) {
        return None;
    }
    OBJECT_TABLE.get(res.handle).ok().map(|e| e.blob_id)
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
    armed: bool,
    last_mask: u32,
    last_edge_gen: u64,
}

pub const MAX_EPOLL_WATCHES_PER_PID: usize = 512;
pub const MAX_EPOLL_WATCHES_GLOBAL: usize = 8192;
const MAX_BLOB_EDGE_ENTRIES: usize = 16384;

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

#[cfg(test)]
pub fn epoll_watch_count() -> usize {
    EPOLL_WATCHES.lock().len()
}

pub fn epoll_has_ready_watch(ep_blob: BlobId, pid: u32) -> bool {
    let snapshot: Vec<EpollWatch> = {
        let watches = EPOLL_WATCHES.lock();
        let mut out = Vec::new();
        for w in watches.iter() {
            if w.pid == pid && w.ep_blob == ep_blob && w.armed && out.try_reserve(1).is_ok() {
                out.push(*w);
            }
        }
        out
    };
    for w in snapshot {
        if let Ok(mask) = fd_readiness_mask(w.fd, pid) {
            let interest = (w.events & (EPOLLIN | EPOLLOUT | EPOLLRDHUP)) | EPOLLERR | EPOLLHUP;
            let active = mask & interest;
            if active == 0 {
                continue;
            }
            if (w.events & EPOLLET) != 0 {
                let cur_edge = fd_blob_id(w.fd, pid).map(blob_edge_gen).unwrap_or(0);
                let effective_last = w.last_mask & active;
                if (active & !effective_last) != 0 || cur_edge != w.last_edge_gen {
                    return true;
                }
            } else {
                return true;
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
            if watches.len() >= MAX_EPOLL_WATCHES_GLOBAL {
                return Err(FsBridgeError::NoSpace);
            }
            let pid_watches = watches.iter().filter(|w| w.pid == pid).count();
            if pid_watches >= MAX_EPOLL_WATCHES_PER_PID {
                return Err(FsBridgeError::NoSpace);
            }
            let ev = parsed_ev.ok_or(FsBridgeError::Invalid)?;
            let ev_events = ev.events;
            let ev_data = ev.data;
            watches
                .try_reserve(1)
                .map_err(|_| FsBridgeError::NoMemory)?;
            watches.push(EpollWatch {
                pid,
                ep_blob: ep_entry.blob_id,
                fd,
                events: ev_events,
                data: ev_data,
                armed: true,
                last_mask: 0,
                last_edge_gen: 0,
            });
        }
        EPOLL_CTL_MOD => {
            let pos = existing_pos.ok_or(FsBridgeError::NotFound)?;
            let ev = parsed_ev.ok_or(FsBridgeError::Invalid)?;
            let ev_events = ev.events;
            let ev_data = ev.data;
            watches[pos].events = ev_events;
            watches[pos].data = ev_data;
            watches[pos].armed = true;
            watches[pos].last_mask = 0;
            watches[pos].last_edge_gen = 0;
        }
        EPOLL_CTL_DEL => {
            let pos = existing_pos.ok_or(FsBridgeError::NotFound)?;
            watches.swap_remove(pos);
        }
        _ => return Err(FsBridgeError::Invalid),
    }
    drop(watches);
    notify_io_event(Some(ep_entry.blob_id));
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
    let timeout_ns = if timeout < 0 {
        None
    } else {
        Some((timeout as u64).saturating_mul(1_000_000))
    };
    fs_epoll_wait_ns(epfd, events_ptr, maxevents, timeout_ns, pid)
}

/// Nanosecond-precision `epoll_wait` / `epoll_pwait2` core.
pub fn fs_epoll_wait_ns(
    epfd: u32,
    events_ptr: u64,
    maxevents: i32,
    timeout_ns: Option<u64>,
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

    let deadline_ns = timeout_ns.map(|ns| {
        crate::scheduler::timer::clock::monotonic_ns().saturating_add(ns)
    });

    loop {
        let gen = io_wait_gen();
        let snapshot: Vec<EpollWatch> = {
            let watches = EPOLL_WATCHES.lock();
            let mut out = Vec::new();
            for w in watches.iter() {
                if w.pid == pid && w.ep_blob == ep_entry.blob_id {
                    if out.try_reserve(1).is_ok() {
                        out.push(*w);
                    }
                }
            }
            out
        };

        let mut ready = 0usize;
        let max_out = (maxevents as usize).min(1024);
        let mut has_external = false;
        let mut updates: Vec<(u32, bool, u32, u64)> = Vec::new();

        for w in snapshot {
            if !w.armed {
                continue;
            }
            if fd_needs_external_poll_slice(w.fd, pid) {
                has_external = true;
            }
            match fd_readiness_mask(w.fd, pid) {
                Ok(mask) => {
                    let interest =
                        (w.events & (EPOLLIN | EPOLLOUT | EPOLLRDHUP)) | EPOLLERR | EPOLLHUP;
                    let active = mask & interest;
                    let is_et = (w.events & EPOLLET) != 0;
                    let cur_edge = if is_et {
                        fd_blob_id(w.fd, pid).map(blob_edge_gen).unwrap_or(0)
                    } else {
                        0
                    };

                    if is_et {
                        // Drop bits that are no longer asserted so future 0->1 transitions trigger.
                        let cleared_last = w.last_mask & active;
                        let edge_fire = active != 0
                            && ((active & !cleared_last) != 0 || cur_edge != w.last_edge_gen);
                        if !edge_fire {
                            if cleared_last != w.last_mask && updates.try_reserve(1).is_ok() {
                                updates.push((w.fd, w.armed, cleared_last, w.last_edge_gen));
                            }
                            continue;
                        }
                    }

                    if active != 0 && ready < max_out {
                        let addr = events_ptr
                            .checked_add(
                                (ready as u64).saturating_mul(size_of::<LinuxEpollEvent>() as u64),
                            )
                            .ok_or(FsBridgeError::Fault)?;
                        write_user_typed(
                            addr,
                            LinuxEpollEvent {
                                events: active,
                                data: w.data,
                            },
                        )
                        .map_err(|_| FsBridgeError::Fault)?;
                        ready += 1;

                        let next_armed = (w.events & EPOLLONESHOT) == 0;
                        let next_last = if is_et { active } else { 0 };
                        let next_edge = if is_et { cur_edge } else { 0 };
                        if (!next_armed || is_et) && updates.try_reserve(1).is_ok() {
                            updates.push((w.fd, next_armed, next_last, next_edge));
                        }
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

        if !updates.is_empty() {
            let mut watches = EPOLL_WATCHES.lock();
            for (u_fd, u_armed, u_last, u_edge) in updates {
                if let Some(w) = watches
                    .iter_mut()
                    .find(|x| x.pid == pid && x.ep_blob == ep_entry.blob_id && x.fd == u_fd)
                {
                    w.armed = u_armed;
                    w.last_mask = u_last;
                    w.last_edge_gen = u_edge;
                }
            }
        }

        if ready > 0 || timeout_ns == Some(0) {
            return Ok(ready as i64);
        }

        let max_slice = if has_external {
            2_000_000 // 2 ms for Ring 1 network_server / TTY descriptors
        } else {
            1_000_000_000 // 1 s — purely event-driven wakeup via notify_io_event
        };
        if !wait_for_io_event(gen, deadline_ns, max_slice)? {
            return Ok(0);
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
    let timeout_ns = if timeout < 0 {
        None
    } else {
        Some((timeout as u64).saturating_mul(1_000_000))
    };
    fs_poll_ns(fds_ptr, nfds, timeout_ns, pid)
}

/// Nanosecond-precision `poll` / `ppoll` core.
pub fn fs_poll_ns(
    fds_ptr: u64,
    nfds: usize,
    timeout_ns: Option<u64>,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if nfds > 1024 {
        return Err(FsBridgeError::Invalid);
    }
    if nfds == 0 {
        match timeout_ns {
            Some(0) => return Ok(0),
            Some(ns) => {
                if !crate::scheduler::timer::sleep::sleep_ns(ns) {
                    return Err(FsBridgeError::Interrupted);
                }
                return Ok(0);
            }
            None => loop {
                if !crate::scheduler::timer::sleep::sleep_ns(1_000_000_000) {
                    return Err(FsBridgeError::Interrupted);
                }
            },
        }
    }
    if fds_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }

    let deadline_ns = timeout_ns.map(|ns| {
        crate::scheduler::timer::clock::monotonic_ns().saturating_add(ns)
    });

    loop {
        let gen = io_wait_gen();
        let mut ready = 0i64;
        let mut has_external = false;
        let mut i = 0usize;
        while i < nfds {
            let addr = fds_ptr
                .checked_add((i as u64).saturating_mul(size_of::<LinuxPollFd>() as u64))
                .ok_or(FsBridgeError::Fault)?;
            let mut pfd = read_user_typed::<LinuxPollFd>(addr).map_err(|_| FsBridgeError::Fault)?;
            pfd.revents = 0;
            if pfd.fd >= 0 {
                if fd_needs_external_poll_slice(pfd.fd as u32, pid) {
                    has_external = true;
                }
                match fd_readiness_mask(pfd.fd as u32, pid) {
                    Ok(mask) => {
                        if (mask & EPOLLIN) != 0 && (pfd.events & POLLIN) != 0 {
                            pfd.revents |= POLLIN;
                        }
                        if (mask & EPOLLOUT) != 0 && (pfd.events & POLLOUT) != 0 {
                            pfd.revents |= POLLOUT;
                        }
                        if (mask & EPOLLRDHUP) != 0 && (pfd.events & POLLRDHUP) != 0 {
                            pfd.revents |= POLLRDHUP;
                        }
                        if (mask & EPOLLERR) != 0 {
                            pfd.revents |= POLLERR;
                        }
                        if (mask & EPOLLHUP) != 0 {
                            pfd.revents |= POLLHUP;
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
        if ready > 0 || timeout_ns == Some(0) {
            return Ok(ready);
        }
        let max_slice = if has_external {
            2_000_000
        } else {
            1_000_000_000
        };
        if !wait_for_io_event(gen, deadline_ns, max_slice)? {
            return Ok(0);
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
    fs_select_ns(nfds, readfds_ptr, writefds_ptr, exceptfds_ptr, timeout_ns, pid)
}

/// Nanosecond-precision `select` / `pselect6` core.
pub fn fs_select_ns(
    nfds: usize,
    readfds_ptr: u64,
    writefds_ptr: u64,
    exceptfds_ptr: u64,
    timeout_ns: Option<u64>,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if nfds > 1024 {
        return Err(FsBridgeError::Invalid);
    }
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

    let deadline_ns = timeout_ns.map(|ns| {
        crate::scheduler::timer::clock::monotonic_ns().saturating_add(ns)
    });
    let mut out_read = vec_with_zeroes(bytes)?;
    let mut out_write = vec_with_zeroes(bytes)?;
    let mut out_except = vec_with_zeroes(bytes)?;

    let ready = loop {
        let gen = io_wait_gen();
        out_read.fill(0);
        out_write.fill(0);
        out_except.fill(0);
        let mut ready = 0i64;
        let mut has_external = false;
        let mut fd = 0usize;
        while fd < nfds {
            let check_r = readfds_ptr != 0 && fdset_bit(&orig_read, fd);
            let check_w = writefds_ptr != 0 && fdset_bit(&orig_write, fd);
            let check_e = exceptfds_ptr != 0 && fdset_bit(&orig_except, fd);
            if check_r || check_w || check_e {
                if fd_needs_external_poll_slice(fd as u32, pid) {
                    has_external = true;
                }
                let mask = fd_readiness_mask(fd as u32, pid)?;
                if check_r && (mask & (EPOLLIN | EPOLLHUP | EPOLLERR)) != 0 {
                    set_fdset_bit(&mut out_read, fd, true);
                    ready += 1;
                }
                if check_w && (mask & (EPOLLOUT | EPOLLERR)) != 0 {
                    set_fdset_bit(&mut out_write, fd, true);
                    ready += 1;
                }
                if check_e && (mask & EPOLLERR) != 0 {
                    set_fdset_bit(&mut out_except, fd, true);
                    ready += 1;
                }
            }
            fd += 1;
        }
        if ready > 0 || timeout_ns == Some(0) {
            break ready;
        }
        let max_slice = if has_external {
            2_000_000
        } else {
            1_000_000_000
        };
        if !wait_for_io_event(gen, deadline_ns, max_slice)? {
            break 0;
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
