//! # syscall/table/fs/poll_event.rs — Handlers de multiplexage I/O, tubes et événements

use crate::syscall::numbers::*;
use crate::syscall::table::common::*;
use crate::syscall::validation::validate_fd;

/// `pipe(pipefd)`.
pub fn sys_pipe(fds_ptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_PIPE);
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_pipe2(fds_ptr, 0, pid))
}

/// `pipe2(pipefd, flags)`.
pub fn sys_pipe2(fds_ptr: u64, flags: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_PIPE2);
    let flags = match checked_u32_sysarg(flags) {
        Ok(v) => v,
        Err(e) => return e,
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_pipe2(fds_ptr, flags, pid))
}

/// `eventfd(initval)`.
pub fn sys_eventfd(initval: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EVENTFD);
    let initval = match checked_u32_sysarg(initval) {
        Ok(v) => v,
        Err(e) => return e,
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_eventfd2(initval, 0, pid))
}

/// `eventfd2(initval, flags)`.
pub fn sys_eventfd2(initval: u64, flags: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EVENTFD2);
    let initval = match checked_u32_sysarg(initval) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let flags = match checked_u32_sysarg(flags) {
        Ok(v) => v,
        Err(e) => return e,
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_eventfd2(initval, flags, pid))
}

/// `inotify_init1(flags)`.
pub fn sys_inotify_init1(flags: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_INOTIFY_INIT1);
    let flags = match checked_u32_sysarg(flags) {
        Ok(v) => v,
        Err(e) => return e,
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_inotify_init1(flags, pid))
}

/// `poll(fds, nfds, timeout)`.
pub fn sys_poll(fds_ptr: u64, nfds: u64, timeout: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_POLL);
    if nfds > 1024 {
        return EINVAL;
    }
    let timeout = match checked_i32_sysarg(timeout) {
        Ok(v) => v,
        Err(e) => return e,
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_poll(fds_ptr, nfds as usize, timeout, pid))
}

/// `ppoll(fds, nfds, timeout, sigmask, sigsetsize)`.
pub fn sys_ppoll(
    fds_ptr: u64,
    nfds: u64,
    timeout_ptr: u64,
    sigmask_ptr: u64,
    sigsetsize: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_PPOLL);
    let _ = (timeout_ptr, sigmask_ptr, sigsetsize);
    sys_poll(fds_ptr, nfds, 0, 0, 0, 0)
}

/// `select(nfds, readfds, writefds, exceptfds, timeout)`.
pub fn sys_select(
    nfds: u64,
    readfds_ptr: u64,
    writefds_ptr: u64,
    exceptfds_ptr: u64,
    timeout_ptr: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_SELECT);
    if nfds > 1024 {
        return EINVAL;
    }
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_select(
        nfds as usize,
        readfds_ptr,
        writefds_ptr,
        exceptfds_ptr,
        timeout_ptr,
        pid,
    ))
}

/// `pselect6(nfds, readfds, writefds, exceptfds, timeout, sigmask_pack)`.
pub fn sys_pselect6(
    nfds: u64,
    readfds_ptr: u64,
    writefds_ptr: u64,
    exceptfds_ptr: u64,
    timeout_ptr: u64,
    sigmask_pack: u64,
) -> i64 {
    stat_inc(SYS_PSELECT6);
    let _ = sigmask_pack;
    sys_select(
        nfds,
        readfds_ptr,
        writefds_ptr,
        exceptfds_ptr,
        timeout_ptr,
        0,
    )
}

/// `epoll_create(size)`.
pub fn sys_epoll_create(size: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EPOLL_CREATE);
    if size == 0 {
        return EINVAL;
    }
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_epoll_create1(0, pid))
}

/// `epoll_create1(flags)`.
pub fn sys_epoll_create1(flags: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EPOLL_CREATE1);
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_epoll_create1(flags as u32, pid))
}

/// `epoll_ctl(epfd, op, fd, event)`.
pub fn sys_epoll_ctl(epfd: u64, op: u64, fd: u64, event_ptr: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EPOLL_CTL);
    let epfd = match validate_fd(epfd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_epoll_ctl(
        epfd as u32,
        op as i32,
        fd as u32,
        event_ptr,
        pid,
    ))
}

/// `epoll_wait(epfd, events, maxevents, timeout)`.
pub fn sys_epoll_wait(
    epfd: u64,
    events_ptr: u64,
    maxevents: u64,
    timeout: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EPOLL_WAIT);
    let epfd = match validate_fd(epfd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_epoll_wait(
        epfd as u32,
        events_ptr,
        maxevents as i32,
        timeout as i32,
        pid,
    ))
}

/// `epoll_pwait(epfd, events, maxevents, timeout, sigmask, sigsetsize)`.
pub fn sys_epoll_pwait(
    epfd: u64,
    events_ptr: u64,
    maxevents: u64,
    timeout: u64,
    sigmask_ptr: u64,
    sigsetsize: u64,
) -> i64 {
    stat_inc(SYS_EPOLL_PWAIT);
    let _ = (sigmask_ptr, sigsetsize);
    sys_epoll_wait(epfd, events_ptr, maxevents, timeout, 0, 0)
}

/// `epoll_pwait2(epfd, events, maxevents, timeout, sigmask, sigsetsize)`.
pub fn sys_epoll_pwait2(
    epfd: u64,
    events_ptr: u64,
    maxevents: u64,
    timeout_ptr: u64,
    sigmask_ptr: u64,
    sigsetsize: u64,
) -> i64 {
    stat_inc(SYS_EPOLL_PWAIT2);
    let _ = (timeout_ptr, sigmask_ptr, sigsetsize);
    sys_epoll_wait(epfd, events_ptr, maxevents, 0, 0, 0)
}
