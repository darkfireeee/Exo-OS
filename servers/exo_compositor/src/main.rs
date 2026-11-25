//! # `exo_compositor` — Isolated Strate 2 Wayland Compositor (S5B)
//!
//! Zero-Trust architectural split from `fb_server`:
//! - Runs in Rust `std` (`x86_64-unknown-exo`) with **ZERO MMIO** and **ZERO hardware privileges**.
//! - Hosts `/run/wayland-0` (`AF_UNIX` + `SCM_RIGHTS` + `epoll`).
//! - Enforces `F_SEAL_SHRINK`, `fstat` backing size validation, checked arithmetic
//!   `(offset + height * stride) <= pool_size`, and strict per-client quotas.
//! - Submits composited damage strictly via typed `SurfaceGrant` + `DamageRect` IPC
//!   messages to `fb_server` over `ExoCordon` (`ServiceClass::ExoCompositor -> FbServer`).
//! - Belongs to **ExoPhoenix Strate 2** (transient, restartable by `init_server` without
//!   impacting `fb_server` or the 1 s SLA).

mod fb_grant_client;
mod shm_pool;
mod wl_protocol;

use exo_syscall_abi as syscall;
use fb_grant_client::{register_ipc_endpoint, request_fb_surface_grant};
pub use shm_pool::{
    validate_shm_buffer_bounds, MAX_POOLS_PER_CLIENT, MAX_SHM_BYTES_PER_CLIENT,
};
pub use wl_protocol::{MAX_OBJECTS_PER_CLIENT, MAX_WIRE_MSG_SIZE};
use wl_protocol::{
    bind_wayland_listener, dispatch_client_messages, ClientState, RawIoVec, UnixCmsghdr,
    UnixMsghdr,
};

pub const MAX_WAYLAND_CLIENTS: usize = 8;
pub const MAX_RX_BUF: usize = 8192;

const MSG_DONTWAIT: u64 = 0x40;
const EINTR_ERR: i64 = -4;
const EPOLL_CTL_ADD: u64 = 1;
const EPOLL_CTL_DEL: u64 = 2;
const EPOLLIN: u32 = 0x0001;

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
struct EpollEvent {
    events: u32,
    data: u64,
}

fn main() {
    syscall::boot_log(b"exo_compositor: boot (Strate 2 isolated Wayland compositor, 0 MMIO)\n");
    let mut listen_fd = None;
    for _ in 0..20 {
        if let Some(fd) = bind_wayland_listener() {
            listen_fd = Some(fd);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let Some(listen_fd) = listen_fd else {
        syscall::boot_log(b"exo_compositor: failed to bind /run/wayland-0\n");
        std::process::exit(1);
    };
    let Some((pid, ep)) = register_ipc_endpoint() else {
        syscall::boot_log(b"exo_compositor: failed to register IPC endpoint\n");
        std::process::exit(1);
    };
    syscall::boot_log(b"exo_compositor: /run/wayland-0 ready\n");

    let epfd = unsafe { syscall::syscall1(syscall::SYS_EPOLL_CREATE1, 0) } as i32;
    if epfd >= 0 {
        let ev = EpollEvent {
            events: EPOLLIN,
            data: listen_fd as u64,
        };
        let _ = unsafe {
            syscall::syscall4(
                syscall::SYS_EPOLL_CTL,
                epfd as u64,
                EPOLL_CTL_ADD,
                listen_fd as u64,
                &ev as *const EpollEvent as u64,
            )
        };
    }

    let mut fb_grant = request_fb_surface_grant(pid, ep);
    let mut clients: Vec<ClientState> = Vec::new();
    let mut events = [EpollEvent::default(); 16];

    loop {
        if fb_grant.is_none() {
            fb_grant = request_fb_surface_grant(pid, ep);
        }

        if epfd >= 0 {
            let _ = unsafe {
                syscall::syscall4(
                    syscall::SYS_EPOLL_WAIT,
                    epfd as u64,
                    events.as_mut_ptr() as u64,
                    events.len() as u64,
                    16,
                )
            };
        } else {
            std::thread::sleep(std::time::Duration::from_millis(8));
        }

        // Accept new Wayland clients up to MAX_WAYLAND_CLIENTS.
        loop {
            let cfd = unsafe { syscall::syscall3(syscall::SYS_ACCEPT, listen_fd as u64, 0, 0) };
            if cfd < 0 {
                break;
            }
            if clients.len() >= MAX_WAYLAND_CLIENTS {
                unsafe {
                    let _ = syscall::syscall1(syscall::SYS_CLOSE, cfd as u64);
                }
                continue;
            }
            if epfd >= 0 {
                let ev = EpollEvent {
                    events: EPOLLIN,
                    data: cfd as u64,
                };
                let _ = unsafe {
                    syscall::syscall4(
                        syscall::SYS_EPOLL_CTL,
                        epfd as u64,
                        EPOLL_CTL_ADD,
                        cfd as u64,
                        &ev as *const EpollEvent as u64,
                    )
                };
            }
            clients.push(ClientState::new(cfd as i32));
        }

        // Poll active clients.
        let mut idx = 0usize;
        while idx < clients.len() {
            let mut disconnected = false;
            {
                let client = &mut clients[idx];
                let mut tmp = [0u8; 2048];
                for _ in 0..4 {
                    if client.rx_buf.len() >= MAX_RX_BUF {
                        disconnected = true;
                        break;
                    }
                    let mut iov = RawIoVec {
                        iov_base: tmp.as_mut_ptr() as u64,
                        iov_len: tmp.len() as u64,
                    };
                    let mut ctrl = [0u8; 128];
                    let mut msg = UnixMsghdr {
                        msg_name: 0,
                        msg_namelen: 0,
                        _pad0: 0,
                        msg_iov: &mut iov as *mut RawIoVec as u64,
                        msg_iovlen: 1,
                        msg_control: ctrl.as_mut_ptr() as u64,
                        msg_controllen: ctrl.len() as u64,
                        msg_flags: 0,
                        _pad1: 0,
                    };
                    let rc = unsafe {
                        syscall::syscall3(
                            syscall::SYS_RECVMSG,
                            client.fd as u64,
                            &mut msg as *mut UnixMsghdr as u64,
                            MSG_DONTWAIT,
                        )
                    };
                    if rc < 0 {
                        if rc != syscall::EAGAIN && rc != EINTR_ERR {
                            disconnected = true;
                        }
                        break;
                    }
                    if msg.msg_controllen >= core::mem::size_of::<UnixCmsghdr>() as u64 {
                        let cmsg =
                            unsafe { core::ptr::read_unaligned(ctrl.as_ptr() as *const UnixCmsghdr) };
                        if cmsg.cmsg_level == 1 && cmsg.cmsg_type == 1 && cmsg.cmsg_len >= 16 {
                            let avail = (msg.msg_controllen as usize)
                                .min(ctrl.len())
                                .min(cmsg.cmsg_len);
                            let n_fds = avail.saturating_sub(16) / 4;
                            for fi in 0..n_fds {
                                let off = 16 + fi * 4;
                                let rfd = i32::from_le_bytes([
                                    ctrl[off],
                                    ctrl[off + 1],
                                    ctrl[off + 2],
                                    ctrl[off + 3],
                                ]);
                                if client.rx_fds.len() < 16 {
                                    client.rx_fds.push_back(rfd);
                                } else if rfd >= 0 {
                                    unsafe {
                                        let _ = syscall::syscall1(syscall::SYS_CLOSE, rfd as u64);
                                    }
                                }
                            }
                        }
                    }
                    if rc == 0 && msg.msg_controllen == 0 {
                        disconnected = true;
                        break;
                    }
                    client.rx_buf.extend_from_slice(&tmp[..rc as usize]);
                }

                if !disconnected && client.rx_buf.len() >= 8 {
                    if !dispatch_client_messages(client, pid, fb_grant.as_ref()) {
                        disconnected = true;
                    }
                }

                if !disconnected && !client.tx_buf.is_empty() {
                    let iov = RawIoVec {
                        iov_base: client.tx_buf.as_ptr() as u64,
                        iov_len: client.tx_buf.len() as u64,
                    };
                    let msg = UnixMsghdr {
                        msg_name: 0,
                        msg_namelen: 0,
                        _pad0: 0,
                        msg_iov: &iov as *const RawIoVec as u64,
                        msg_iovlen: 1,
                        msg_control: 0,
                        msg_controllen: 0,
                        msg_flags: 0,
                        _pad1: 0,
                    };
                    let sent = unsafe {
                        syscall::syscall3(
                            syscall::SYS_SENDMSG,
                            client.fd as u64,
                            &msg as *const UnixMsghdr as u64,
                            MSG_DONTWAIT,
                        )
                    };
                    if sent > 0 {
                        let s = (sent as usize).min(client.tx_buf.len());
                        client.tx_buf.drain(..s);
                    } else if sent < 0 && sent != syscall::EAGAIN && sent != EINTR_ERR {
                        disconnected = true;
                    }
                }
            }

            if disconnected {
                let cfd = clients[idx].fd;
                if epfd >= 0 && cfd >= 0 {
                    let _ = unsafe {
                        syscall::syscall4(
                            syscall::SYS_EPOLL_CTL,
                            epfd as u64,
                            EPOLL_CTL_DEL,
                            cfd as u64,
                            0,
                        )
                    };
                }
                let mut removed = clients.swap_remove(idx);
                removed.cleanup();
            } else {
                idx += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wl_protocol::ObjKind;

    #[test]
    fn buffer_bounds_rejects_stride_and_offset_overflows() {
        assert!(validate_shm_buffer_bounds(0, 64, 64, 256, 4, 64 * 64 * 4));
        assert!(!validate_shm_buffer_bounds(0, 64, 64, 128, 4, 64 * 64 * 4));
        assert!(!validate_shm_buffer_bounds(0, 64, 64, u32::MAX, 4, 16 * 1024 * 1024));
        assert!(!validate_shm_buffer_bounds(u32::MAX, 64, 64, 256, 4, 16 * 1024 * 1024));
        assert!(!validate_shm_buffer_bounds(4096, 64, 64, 256, 4, 64 * 64 * 4));
    }

    #[test]
    fn client_object_quota_enforced_without_panic() {
        let mut client = ClientState::new(-1);
        for id in 2..(MAX_OBJECTS_PER_CLIENT as u32 + 1) {
            assert!(client.register_obj(id, ObjKind::Region));
        }
        assert!(!client.register_obj(999, ObjKind::Region));
    }
}
