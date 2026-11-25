//! # syscall/fs_bridge/special/unix_socket/ancillary_fds.rs — `sendmsg`/`recvmsg` & `SCM_RIGHTS` policy (S3)

use alloc::vec::Vec;
use core::mem::size_of;

use crate::fs::exofs::core::BlobId;
use crate::fs::exofs::syscall::object_fd::OBJECT_TABLE;
use crate::syscall::fs_bridge::*;
use crate::syscall::validation::{copy_to_user, read_user_typed, write_user_typed};

use super::state::{
    append_socket_payload_locked, is_socket_peer_closed_locked, parse_socket_peer_bytes,
    read_socket_payload_locked,
};

/// S3 strict inflight ancillary FD quotas (anti-DoS / anti-FD-exhaustion).
pub const MAX_INFLIGHT_FDS_PER_SOCKET: usize = 16;
pub const MAX_INFLIGHT_FDS_PER_PID: usize = 64;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct UnixMsghdr {
    msg_name: u64,
    msg_namelen: u32,
    _pad0: u32,
    msg_iov: u64,
    msg_iovlen: u64,
    msg_control: u64,
    msg_controllen: u64,
    msg_flags: i32,
    _pad1: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct UnixCmsghdr {
    cmsg_len: usize,
    cmsg_level: i32,
    cmsg_type: i32,
}

fn rollback_staged_ancillary(staged: &[(BlobId, u32, u32, u32)]) {
    for &(_, h, _, _) in staged {
        let _ = close_object_handle_with_cleanup(h);
    }
}

/// S3 Zero-Trust policy enforcement for `SCM_RIGHTS` FD delegation.
fn check_scm_rights_allowed(
    sender_pid: u32,
    peer_blob: BlobId,
    src_res: &ResolvedFd,
) -> Result<(), FsBridgeError> {
    let push_alert = |code: u64| {
        crate::security::shield_feed::push_event(
            sender_pid,
            crate::security::shield_feed::event_type::SYSCALL,
            crate::security::shield_feed::severity::MEDIUM,
            crate::syscall::numbers::SYS_SENDMSG as u32,
            src_res.handle as u64,
            code,
        );
    };

    if is_tty_handle(src_res.handle) || (src_res.flags & FD_NO_DELEGATE) != 0 {
        push_alert(0x5C01);
        return Err(FsBridgeError::OpNotPermitted);
    }
    let src_entry = OBJECT_TABLE
        .get(src_res.handle)
        .map_err(exofs_to_bridge_error)?;
    if (src_entry.flags & FD_NO_DELEGATE) != 0 {
        push_alert(0x5C02);
        return Err(FsBridgeError::OpNotPermitted);
    }

    if is_pseudo_blob(&src_entry.blob_id, PSEUDO_SOCKET_TAG) {
        push_alert(0x5C03);
        return Err(FsBridgeError::OpNotPermitted);
    }

    if is_pseudo_blob(&src_entry.blob_id, PSEUDO_EPOLL_TAG)
        || is_pseudo_blob(&src_entry.blob_id, PSEUDO_INOTIFY_TAG)
    {
        push_alert(0x5C04);
        return Err(FsBridgeError::OpNotPermitted);
    }

    let receiver_pid = UNIX_SOCKET_META
        .lock()
        .iter()
        .find(|m| m.blob_id == peer_blob)
        .map(|m| m.cred_pid)
        .unwrap_or(sender_pid);

    let sender_trust = crate::security::zero_trust::trust_for_pid(sender_pid);
    let receiver_trust = crate::security::zero_trust::trust_for_pid(receiver_pid);
    if sender_trust == crate::security::zero_trust::TrustLevel::Untrusted
        || receiver_trust == crate::security::zero_trust::TrustLevel::Untrusted
    {
        push_alert(0x5C05);
        return Err(FsBridgeError::OpNotPermitted);
    }

    if sender_pid != receiver_pid {
        use crate::security::ipc_policy::{
            check_direct_ipc, service_class_of, IpcPolicyResult, ServiceClass,
        };
        let sender_class = service_class_of(crate::process::core::pid::Pid(sender_pid));
        let receiver_class = service_class_of(crate::process::core::pid::Pid(receiver_pid));
        if sender_class != ServiceClass::Unknown
            && receiver_class != ServiceClass::Unknown
            && check_direct_ipc(
                crate::process::core::pid::Pid(sender_pid),
                crate::process::core::pid::Pid(receiver_pid),
            ) != IpcPolicyResult::Allowed
        {
            push_alert(0x5C06);
            return Err(FsBridgeError::OpNotPermitted);
        }
        let sender_restrictions = crate::security::zero_trust::process_restrictions(sender_pid);
        if (sender_restrictions
            & (crate::security::zero_trust::restriction_flags::PLEDGE_ACTIVE
                | crate::security::zero_trust::restriction_flags::NO_RING1_CRITICAL_IPC))
            != 0
            && matches!(
                receiver_class,
                ServiceClass::InitServer
                    | ServiceClass::CryptoServer
                    | ServiceClass::MemoryServer
                    | ServiceClass::DeviceServer
                    | ServiceClass::ExoShield
            )
        {
            push_alert(0x5C07);
            return Err(FsBridgeError::OpNotPermitted);
        }
    }

    Ok(())
}

fn rollback_received_fds(pid: u32, received: &[(i32, u32)], has_fd_table: bool) {
    for &(rfd, obj_handle) in received {
        if has_fd_table {
            if let Some(h) = close_process_fd(pid, rfd as u32) {
                if h <= u32::MAX as u64 {
                    let _ = close_object_handle_with_cleanup(h as u32);
                }
            } else {
                let _ = close_object_handle_with_cleanup(obj_handle);
            }
        } else {
            let _ = close_object_handle_with_cleanup(obj_handle);
        }
    }
}

/// `sendmsg` on `AF_UNIX` socket with `SCM_RIGHTS` support.
pub fn fs_unix_sendmsg(
    fd: u32,
    msg_ptr: u64,
    flags: u32,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if msg_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    let resolved = resolve_fd(pid, fd)?;
    let entry = OBJECT_TABLE
        .get(resolved.handle)
        .map_err(exofs_to_bridge_error)?;
    if !is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG) {
        return Err(FsBridgeError::Invalid);
    }
    let nonblock = ((resolved.flags | entry.flags) & O_NONBLOCK) != 0 || (flags & 0x40) != 0;
    let nosignal = (flags & 0x4000) != 0;
    if socket_is_write_shutdown(entry.blob_id) {
        if !nosignal {
            let _ = crate::process::signal::delivery::send_signal_to_pid(
                crate::process::core::pid::Pid(pid),
                crate::process::signal::default::Signal::SIGPIPE,
            );
        }
        return Err(FsBridgeError::BrokenPipe);
    }
    let peer = socket_peer_blob(entry.blob_id)?;
    if peer == BlobId([0u8; 32]) {
        return Err(FsBridgeError::NotConnected);
    }
    if is_socket_closed(peer) {
        if !nosignal {
            let _ = crate::process::signal::delivery::send_signal_to_pid(
                crate::process::core::pid::Pid(pid),
                crate::process::signal::default::Signal::SIGPIPE,
            );
        }
        return Err(FsBridgeError::BrokenPipe);
    }
    let msg = read_user_typed::<UnixMsghdr>(msg_ptr).map_err(|_| FsBridgeError::Fault)?;
    if msg.msg_iovlen > 1024 {
        return Err(FsBridgeError::Invalid);
    }

    let mut payload = Vec::new();
    for i in 0..msg.msg_iovlen {
        let addr = msg
            .msg_iov
            .checked_add(i.saturating_mul(size_of::<LinuxIovec>() as u64))
            .ok_or(FsBridgeError::Fault)?;
        let iov = read_user_typed::<LinuxIovec>(addr).map_err(|_| FsBridgeError::Fault)?;
        if iov.iov_len != 0 {
            let chunk = read_user_bytes(iov.iov_base, iov.iov_len as usize)?;
            payload
                .try_reserve(chunk.len())
                .map_err(|_| FsBridgeError::NoMemory)?;
            payload.extend_from_slice(&chunk);
        }
    }

    let mut staged_fds: Vec<(BlobId, u32, u32, u32)> = Vec::new();
    if msg.msg_control != 0 && msg.msg_controllen >= 16 {
        let ctrl_len = (msg.msg_controllen as usize).min(4096);
        let ctrl_bytes = read_user_bytes(msg.msg_control, ctrl_len)?;
        let mut off = 0usize;
        while off + 16 <= ctrl_bytes.len() {
            let hdr = unsafe {
                core::ptr::read_unaligned(ctrl_bytes.as_ptr().add(off) as *const UnixCmsghdr)
            };
            if hdr.cmsg_len < 16 || off + hdr.cmsg_len > ctrl_bytes.len() {
                rollback_staged_ancillary(&staged_fds);
                return Err(FsBridgeError::Invalid);
            }
            if hdr.cmsg_level == 1 && hdr.cmsg_type == 1 {
                let fd_bytes = hdr.cmsg_len - 16;
                let fd_count = fd_bytes / 4;
                if staged_fds.len().saturating_add(fd_count) > MAX_INFLIGHT_FDS_PER_SOCKET {
                    rollback_staged_ancillary(&staged_fds);
                    crate::security::shield_feed::push_event(
                        pid,
                        crate::security::shield_feed::event_type::SYSCALL,
                        crate::security::shield_feed::severity::MEDIUM,
                        crate::syscall::numbers::SYS_SENDMSG as u32,
                        fd_count as u64,
                        0x5C10,
                    );
                    return Err(FsBridgeError::TooManyFds);
                }
                for i in 0..fd_count {
                    let pos = off + 16 + i * 4;
                    let raw_fd = i32::from_ne_bytes([
                        ctrl_bytes[pos],
                        ctrl_bytes[pos + 1],
                        ctrl_bytes[pos + 2],
                        ctrl_bytes[pos + 3],
                    ]);
                    if raw_fd < 0 {
                        rollback_staged_ancillary(&staged_fds);
                        return Err(FsBridgeError::BadFd);
                    }
                    let Ok(src_res) = resolve_fd(pid, raw_fd as u32) else {
                        rollback_staged_ancillary(&staged_fds);
                        return Err(FsBridgeError::BadFd);
                    };
                    if let Err(err) = check_scm_rights_allowed(pid, peer, &src_res) {
                        rollback_staged_ancillary(&staged_fds);
                        return Err(err);
                    }
                    let Ok(dup_handle) = OBJECT_TABLE.dup_checked_for_owner(src_res.handle, 0)
                    else {
                        rollback_staged_ancillary(&staged_fds);
                        return Err(FsBridgeError::BadFd);
                    };
                    if staged_fds.try_reserve(1).is_err() {
                        let _ = close_object_handle_with_cleanup(dup_handle);
                        rollback_staged_ancillary(&staged_fds);
                        return Err(FsBridgeError::NoMemory);
                    }
                    staged_fds.push((peer, dup_handle, src_res.flags, pid));
                }
            }
            off += (hdr.cmsg_len + 7) & !7;
        }
    }

    loop {
        let gen = io_wait_gen();
        let _guard = SOCKET_PAYLOAD_LOCK.lock();
        if socket_is_write_shutdown(entry.blob_id) || is_socket_closed(peer) {
            drop(_guard);
            rollback_staged_ancillary(&staged_fds);
            if !nosignal {
                let _ = crate::process::signal::delivery::send_signal_to_pid(
                    crate::process::core::pid::Pid(pid),
                    crate::process::signal::default::Signal::SIGPIPE,
                );
            }
            return Err(FsBridgeError::BrokenPipe);
        }
        let result = if staged_fds.is_empty() {
            append_socket_payload_locked(peer, &payload)
        } else {
            let mut q = SOCKET_ANCILLARY_FDS.lock();
            let existing_sock = q.iter().filter(|(b, _, _, _)| *b == peer).count();
            let existing_pid = q.iter().filter(|(_, _, _, sp)| *sp == pid).count();
            if existing_sock.saturating_add(staged_fds.len()) > MAX_INFLIGHT_FDS_PER_SOCKET
                || existing_pid.saturating_add(staged_fds.len()) > MAX_INFLIGHT_FDS_PER_PID
            {
                crate::security::shield_feed::push_event(
                    pid,
                    crate::security::shield_feed::event_type::SYSCALL,
                    crate::security::shield_feed::severity::MEDIUM,
                    crate::syscall::numbers::SYS_SENDMSG as u32,
                    (existing_sock.max(existing_pid)) as u64,
                    0x5C11,
                );
                Err(FsBridgeError::TooManyFds)
            } else if q.try_reserve(staged_fds.len()).is_err() {
                Err(FsBridgeError::NoMemory)
            } else {
                match append_socket_payload_locked(peer, &payload) {
                    Ok(n) => {
                        q.extend(staged_fds.drain(..));
                        Ok(n)
                    }
                    Err(err) => Err(err),
                }
            }
        };
        drop(_guard);
        match result {
            Ok(written) => {
                notify_io_event(Some(peer));
                return Ok(written);
            }
            Err(FsBridgeError::WouldBlock) if !nonblock => {
                if let Err(err) = wait_for_io_event(gen, None, 1_000_000_000) {
                    rollback_staged_ancillary(&staged_fds);
                    return Err(err);
                }
            }
            Err(err) => {
                rollback_staged_ancillary(&staged_fds);
                return Err(err);
            }
        }
    }
}

fn fail_recvmsg(
    guard: spin::mutex::MutexGuard<'_, ()>,
    pid: u32,
    received: &[(i32, u32)],
    has_fd_table: bool,
    err: FsBridgeError,
) -> FsBridgeError {
    drop(guard);
    rollback_received_fds(pid, received, has_fd_table);
    err
}

/// `recvmsg` on `AF_UNIX` socket with `SCM_RIGHTS` support.
pub fn fs_unix_recvmsg(
    fd: u32,
    msg_ptr: u64,
    flags: u32,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if msg_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    let resolved = resolve_fd(pid, fd)?;
    let entry = OBJECT_TABLE
        .get(resolved.handle)
        .map_err(exofs_to_bridge_error)?;
    if !is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG) {
        return Err(FsBridgeError::Invalid);
    }
    let mut msg = read_user_typed::<UnixMsghdr>(msg_ptr).map_err(|_| FsBridgeError::Fault)?;
    if msg.msg_iovlen > 1024 {
        return Err(FsBridgeError::Invalid);
    }

    let mut total_cap = 0usize;
    for i in 0..msg.msg_iovlen {
        let addr = msg
            .msg_iov
            .checked_add(i.saturating_mul(size_of::<LinuxIovec>() as u64))
            .ok_or(FsBridgeError::Fault)?;
        let iov = read_user_typed::<LinuxIovec>(addr).map_err(|_| FsBridgeError::Fault)?;
        total_cap = total_cap.saturating_add(iov.iov_len as usize);
    }

    let consume = (flags & 0x02) == 0;
    let nonblock = ((resolved.flags | entry.flags) & O_NONBLOCK) != 0 || (flags & 0x40) != 0;
    loop {
        let gen = io_wait_gen();
        let _guard = SOCKET_PAYLOAD_LOCK.lock();
        let data = read_socket_payload_locked(entry.blob_id, total_cap, false)?;
        let matching: Vec<(u32, u32)> = {
            let q = SOCKET_ANCILLARY_FDS.lock();
            q.iter()
                .filter(|(b, _, _, _)| *b == entry.blob_id)
                .map(|&(_, h, f, _)| (h, f))
                .collect()
        };
        if data.is_empty() && matching.is_empty() {
            let peer_closed = is_socket_peer_closed_locked(entry.blob_id)
                || socket_is_read_shutdown(entry.blob_id);
            drop(_guard);
            if peer_closed {
                msg.msg_controllen = 0;
                msg.msg_flags = 0;
                write_user_typed(msg_ptr, msg).map_err(|_| FsBridgeError::Fault)?;
                return Ok(0);
            }
            if nonblock {
                return Err(FsBridgeError::WouldBlock);
            }
            let _ = wait_for_io_event(gen, None, 1_000_000_000)?;
            continue;
        }

        let mut copied = 0usize;
        for i in 0..msg.msg_iovlen {
            if copied >= data.len() {
                break;
            }
            let addr = msg
                .msg_iov
                .checked_add(i.saturating_mul(size_of::<LinuxIovec>() as u64))
                .ok_or(FsBridgeError::Fault)?;
            let iov = read_user_typed::<LinuxIovec>(addr).map_err(|_| FsBridgeError::Fault)?;
            let chunk = (data.len() - copied).min(iov.iov_len as usize);
            if chunk != 0 {
                copy_to_user(
                    iov.iov_base as *mut u8,
                    data[copied..copied + chunk].as_ptr(),
                    chunk,
                )
                .map_err(|_| FsBridgeError::Fault)?;
                copied += chunk;
            }
        }

        let has_fd_table = process_has_fd_table(pid);
        let mut received_fds: Vec<(i32, u32)> = Vec::new();
        let mut out_msg_flags: i32 = 0;
        let mut delivered_fd_count = 0usize;
        if consume && !matching.is_empty() {
            let max_fds = if msg.msg_control != 0 && msg.msg_controllen >= 16 {
                ((msg.msg_controllen as usize).saturating_sub(16)) / 4
            } else {
                0
            };
            let take = matching.len().min(max_fds);
            if matching.len() > take {
                out_msg_flags |= 0x08;
            }
            for &(obj_handle, fd_flags) in matching.iter().take(take) {
                let dup_handle = match OBJECT_TABLE.dup_transferred(obj_handle, pid as u64) {
                    Ok(h) => h,
                    Err(err) => {
                        return Err(fail_recvmsg(
                            _guard,
                            pid,
                            &received_fds,
                            has_fd_table,
                            exofs_to_bridge_error(err),
                        ));
                    }
                };
                let new_flags = if (flags & 0x4000_0000) != 0 {
                    fd_flags | O_CLOEXEC
                } else {
                    fd_flags
                };
                let new_fd = if has_fd_table {
                    let Some(installed) = install_process_fd(pid, dup_handle as u64, new_flags)
                    else {
                        drop(_guard);
                        let _ = close_object_handle_with_cleanup(dup_handle);
                        rollback_received_fds(pid, &received_fds, has_fd_table);
                        return Err(FsBridgeError::NoMemory);
                    };
                    installed
                } else {
                    dup_handle as i32
                };
                if received_fds.try_reserve(1).is_err() {
                    drop(_guard);
                    rollback_received_fds(pid, &[(new_fd, dup_handle)], has_fd_table);
                    rollback_received_fds(pid, &received_fds, has_fd_table);
                    return Err(FsBridgeError::NoMemory);
                }
                received_fds.push((new_fd, dup_handle));
            }
            delivered_fd_count = received_fds.len();

            if !received_fds.is_empty() {
                let cmsg_len = 16 + received_fds.len() * 4;
                let aligned_len = ((cmsg_len + 7) & !7).min(msg.msg_controllen as usize);
                let mut cmsg_bytes = Vec::new();
                if cmsg_bytes.try_reserve_exact(aligned_len).is_err() {
                    return Err(fail_recvmsg(
                        _guard,
                        pid,
                        &received_fds,
                        has_fd_table,
                        FsBridgeError::NoMemory,
                    ));
                }
                cmsg_bytes.resize(aligned_len, 0);
                cmsg_bytes[0..8].copy_from_slice(&cmsg_len.to_ne_bytes());
                cmsg_bytes[8..12].copy_from_slice(&1i32.to_ne_bytes());
                cmsg_bytes[12..16].copy_from_slice(&1i32.to_ne_bytes());
                for (idx, &(rfd, _)) in received_fds.iter().enumerate() {
                    let off = 16 + idx * 4;
                    cmsg_bytes[off..off + 4].copy_from_slice(&rfd.to_ne_bytes());
                }
                if copy_to_user(
                    msg.msg_control as *mut u8,
                    cmsg_bytes.as_ptr(),
                    aligned_len,
                )
                .is_err()
                {
                    return Err(fail_recvmsg(
                        _guard,
                        pid,
                        &received_fds,
                        has_fd_table,
                        FsBridgeError::Fault,
                    ));
                }
                msg.msg_controllen = aligned_len as u64;
            } else {
                msg.msg_controllen = 0;
            }
        } else {
            msg.msg_controllen = 0;
        }

        msg.msg_flags = out_msg_flags;
        if write_user_typed(msg_ptr, msg).is_err() {
            return Err(fail_recvmsg(
                _guard,
                pid,
                &received_fds,
                has_fd_table,
                FsBridgeError::Fault,
            ));
        }

        let peer = snapshot_blob(&entry.blob_id)
            .ok()
            .and_then(|d| parse_socket_peer_bytes(&d).ok())
            .unwrap_or(BlobId([0u8; 32]));

        if consume {
            if copied != 0 {
                let _ = read_socket_payload_locked(entry.blob_id, copied, true)?;
            }
            if delivered_fd_count > 0 {
                let mut to_close = Vec::new();
                {
                    let mut q = SOCKET_ANCILLARY_FDS.lock();
                    let mut i = 0;
                    let mut removed = 0usize;
                    while i < q.len() && removed < delivered_fd_count {
                        if q[i].0 == entry.blob_id {
                            let (_, obj_handle, _, _) = q.remove(i);
                            if to_close.try_reserve(1).is_ok() {
                                to_close.push(obj_handle);
                            } else {
                                let _ = OBJECT_TABLE.close(obj_handle);
                            }
                            removed += 1;
                        } else {
                            i += 1;
                        }
                    }
                }
                drop(_guard);
                for h in to_close {
                    let _ = close_object_handle_with_cleanup(h);
                }
            } else {
                drop(_guard);
            }
            if peer != BlobId([0u8; 32]) {
                notify_io_event(Some(peer));
            }
        }
        return Ok(copied as i64);
    }
}
