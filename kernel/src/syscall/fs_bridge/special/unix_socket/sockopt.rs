//! # syscall/fs_bridge/special/unix_socket/sockopt.rs — `setsockopt`/`getsockopt` (`SO_PEERCRED`, buffers, timeouts)

use core::mem::size_of;

use crate::fs::exofs::core::BlobId;
use crate::fs::exofs::syscall::object_fd::OBJECT_TABLE;
use crate::syscall::fs_bridge::*;
use crate::syscall::validation::{read_user_typed, write_user_typed};

/// `setsockopt(fd, level, optname, optval, optlen)` on `AF_UNIX` socket.
pub fn fs_unix_setsockopt(
    fd: u32,
    level: i32,
    optname: i32,
    optval: u64,
    optlen: u32,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let resolved = resolve_fd(pid, fd)?;
    let entry = OBJECT_TABLE
        .get(resolved.handle)
        .map_err(exofs_to_bridge_error)?;
    if !is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG) {
        return Err(FsBridgeError::Invalid);
    }
    if optlen != 0 && optval == 0 {
        return Err(FsBridgeError::Fault);
    }
    const SOL_SOCKET: i32 = 1;
    const SO_REUSEADDR: i32 = 2;
    const SO_SNDBUF: i32 = 7;
    const SO_RCVBUF: i32 = 8;
    const SO_PASSCRED: i32 = 16;
    const SO_RCVTIMEO: i32 = 20;
    const SO_SNDTIMEO: i32 = 21;
    if level != SOL_SOCKET {
        return Err(FsBridgeError::NoProtoOpt);
    }
    let mut meta = UNIX_SOCKET_META.lock();
    let Some(m) = meta.iter_mut().find(|m| m.blob_id == entry.blob_id) else {
        return Err(FsBridgeError::BadFd);
    };
    match optname {
        SO_REUSEADDR | SO_PASSCRED | SO_SNDBUF | SO_RCVBUF => {
            if optlen < 4 {
                return Err(FsBridgeError::Invalid);
            }
            let val = read_user_typed::<i32>(optval).map_err(|_| FsBridgeError::Fault)?;
            match optname {
                SO_REUSEADDR => m.reuseaddr = val != 0,
                SO_PASSCRED => m.passcred = val != 0,
                SO_SNDBUF => m.sndbuf = (val.max(4096) as u32).min(SOCKET_MAX_CAPACITY as u32),
                SO_RCVBUF => m.rcvbuf = (val.max(4096) as u32).min(SOCKET_MAX_CAPACITY as u32),
                _ => {}
            }
            Ok(0)
        }
        SO_RCVTIMEO | SO_SNDTIMEO => {
            if optlen < 16 {
                return Err(FsBridgeError::Invalid);
            }
            let _ts = read_user_typed::<LinuxTimeval>(optval).map_err(|_| FsBridgeError::Fault)?;
            Ok(0)
        }
        _ => Err(FsBridgeError::NoProtoOpt),
    }
}

/// `getsockopt(fd, level, optname, optval, optlen*)` on `AF_UNIX` socket.
pub fn fs_unix_getsockopt(
    fd: u32,
    level: i32,
    optname: i32,
    optval: u64,
    optlen_ptr: u64,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let resolved = resolve_fd(pid, fd)?;
    let entry = OBJECT_TABLE
        .get(resolved.handle)
        .map_err(exofs_to_bridge_error)?;
    if !is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG) {
        return Err(FsBridgeError::Invalid);
    }
    if optval == 0 || optlen_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    const SOL_SOCKET: i32 = 1;
    const SO_REUSEADDR: i32 = 2;
    const SO_TYPE: i32 = 3;
    const SO_ERROR: i32 = 4;
    const SO_SNDBUF: i32 = 7;
    const SO_RCVBUF: i32 = 8;
    const SO_PASSCRED: i32 = 16;
    const SO_PEERCRED: i32 = 17;
    if level != SOL_SOCKET {
        return Err(FsBridgeError::NoProtoOpt);
    }
    let user_len = read_user_typed::<u32>(optlen_ptr).map_err(|_| FsBridgeError::Fault)? as usize;
    match optname {
        SO_PEERCRED => {
            #[repr(C)]
            #[derive(Clone, Copy)]
            struct LinuxUcred {
                pid: i32,
                uid: u32,
                gid: u32,
            }
            if user_len < size_of::<LinuxUcred>() {
                return Err(FsBridgeError::Invalid);
            }
            let peer = socket_peer_blob(entry.blob_id)?;
            let cred = {
                let meta = UNIX_SOCKET_META.lock();
                if peer != BlobId([0u8; 32]) {
                    meta.iter()
                        .find(|m| m.blob_id == peer)
                        .map(|m| LinuxUcred {
                            pid: m.cred_pid as i32,
                            uid: m.cred_uid,
                            gid: m.cred_gid,
                        })
                        .unwrap_or(LinuxUcred {
                            pid: pid as i32,
                            uid: 0,
                            gid: 0,
                        })
                } else {
                    LinuxUcred {
                        pid: 0,
                        uid: u32::MAX,
                        gid: u32::MAX,
                    }
                }
            };
            write_user_typed(optval, cred).map_err(|_| FsBridgeError::Fault)?;
            write_user_typed::<u32>(optlen_ptr, size_of::<LinuxUcred>() as u32)
                .map_err(|_| FsBridgeError::Fault)?;
            Ok(0)
        }
        SO_TYPE | SO_ERROR | SO_REUSEADDR | SO_PASSCRED | SO_SNDBUF | SO_RCVBUF => {
            if user_len < 4 {
                return Err(FsBridgeError::Invalid);
            }
            let val: i32 = {
                let mut meta = UNIX_SOCKET_META.lock();
                let Some(m) = meta.iter_mut().find(|m| m.blob_id == entry.blob_id) else {
                    return Err(FsBridgeError::BadFd);
                };
                match optname {
                    SO_TYPE => m.sock_type,
                    SO_ERROR => {
                        let err = m.last_error;
                        m.last_error = 0;
                        err
                    }
                    SO_REUSEADDR => i32::from(m.reuseaddr),
                    SO_PASSCRED => i32::from(m.passcred),
                    SO_SNDBUF => m.sndbuf as i32,
                    SO_RCVBUF => m.rcvbuf as i32,
                    _ => 0,
                }
            };
            write_user_typed::<i32>(optval, val).map_err(|_| FsBridgeError::Fault)?;
            write_user_typed::<u32>(optlen_ptr, 4).map_err(|_| FsBridgeError::Fault)?;
            Ok(0)
        }
        _ => Err(FsBridgeError::NoProtoOpt),
    }
}
