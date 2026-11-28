//! # `exo_compositor` — Isolated Wayland & Desktop Compositor (0 MMIO)
//!
//! Modular architecture:
//! - `assets`, `i18n`, `keymap`: embedded fonts/icons/wallpapers, FR/EN, AZERTY/QWERTY/BÉPO
//! - `telemetry/*`: RTC clock/calendar, live Ring 0/1 counters, ExoFS metadata
//! - `bench/*`: all-module ExoBench profiler & final report generator
//! - `desktop/*`: window manager, topbar, animated centered dock, launcher, dialogs, input, render
//! - `apps/*`: Files, Settings, Monitor+Bench, Terminal, Text Editor, 2D Games

mod apps;
mod assets;
mod bench;
mod desktop;
mod fb_grant_client;
mod i18n;
mod keymap;
mod net_diag;
mod shm_pool;
mod telemetry;
mod wl_protocol;

use desktop::{DesktopShell, PowerRequest};
use exo_syscall_abi as syscall;
use fb_grant_client::{
    attach_input_stream, recv_input_event_nb, recv_input_event_timeout, register_ipc_endpoint,
    request_fb_surface_grant, request_init_power_action, set_keyboard_grab,
    submit_damage_to_fb_server, sync_cursor_to_fb_server,
};
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
    syscall::boot_log(b"exo_compositor: boot (isolated Desktop + Wayland compositor, 0 MMIO)\n");
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
        let ev = EpollEvent { events: EPOLLIN, data: listen_fd as u64 };
        let _ = unsafe {
            syscall::syscall4(syscall::SYS_EPOLL_CTL, epfd as u64, EPOLL_CTL_ADD, listen_fd as u64, &ev as *const EpollEvent as u64)
        };
    }

    let mut fb_grant = request_fb_surface_grant(pid, ep);
    let mut input_ep = attach_input_stream(pid);
    let mut desktop: Option<DesktopShell> = fb_grant.as_ref().map(|g| {
        let mut d = DesktopShell::new(g.width, g.height);
        d.gpu_hw_active = g.gpu_hw_active;
        d
    });
    let mut kbd_grabbed = false;
    let mut last_periodic_ns: u64 = bench::now_ns();
    let mut last_present_ns: u64 = 0;
    let mut fps_window_start_ns: u64 = bench::now_ns();
    let mut fps_frames: u32 = 0;
    const TARGET_FRAME_NS: u64 = 16_666_667; // Exact 60 Hz vblank frame pacing

    let mut clients: Vec<ClientState> = Vec::new();
    let mut events = [EpollEvent::default(); 16];

    let mut presented_initial = false;
    // Present clean initial desktop immediately (0 open windows on boot!)
    if let (Some(grant), Some(desk)) = (fb_grant.as_mut(), desktop.as_mut()) {
        let (gw, gh) = (grant.width, grant.height);
        if let Some(pixels) = grant.pixels_mut() {
            desk.render_rect_to_shm(pixels, exo_ui::Rect::new(0, 0, gw as i32, gh as i32));
            let _ = desk.take_dirty_rects();
            let dmg = syscall::DamageRect { x: 0, y: 0, width: gw, height: gh };
            submit_damage_to_fb_server(pid, ep, grant, dmg);
            last_present_ns = bench::now_ns();
            presented_initial = true;
            syscall::boot_log(b"exo_compositor: Desktop initial frame presented\n");
        }
    }

    loop {
        if fb_grant.is_none() {
            fb_grant = request_fb_surface_grant(pid, ep);
            if let Some(g) = fb_grant.as_ref() {
                let mut d = DesktopShell::new(g.width, g.height);
                d.gpu_hw_active = g.gpu_hw_active;
                desktop = Some(d);
            }
        }
        if input_ep.is_none() {
            input_ep = attach_input_stream(pid);
        }

        // Compute adaptive sleep until next 60 Hz frame boundary so rapid key-repeats coalesce cleanly
        // and animations (power transition / dock) run at a steady 60 Hz without burning CPU.
        let wait_ms: u64 = if let Some(desk) = desktop.as_ref() {
            if desk.has_damage() || desk.is_power_transitioning() {
                let elapsed = bench::now_ns().saturating_sub(last_present_ns);
                if elapsed >= TARGET_FRAME_NS {
                    0
                } else {
                    ((TARGET_FRAME_NS - elapsed) / 1_000_000).clamp(1, 8)
                }
            } else {
                8
            }
        } else {
            8
        };

        // Wake INSTANTLY (< 0.1 ms) on mouse/keyboard IPC arrival; max 8 ms idle tick for smooth 60 Hz!
        let first_ev = if let Some(iep) = input_ep {
            if wait_ms == 0 {
                recv_input_event_nb(iep)
            } else {
                recv_input_event_timeout(iep, wait_ms)
            }
        } else {
            if epfd >= 0 {
                let _ = unsafe {
                    syscall::syscall4(syscall::SYS_EPOLL_WAIT, epfd as u64, events.as_mut_ptr() as u64, events.len() as u64, wait_ms)
                };
            } else if wait_ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(wait_ms));
            }
            None
        };

        if let (Some(iep), Some(desk)) = (input_ep, desktop.as_mut()) {
            let (prev_mx, prev_my) = (desk.mouse_x, desk.mouse_y);
            let mut acc_dx: i32 = 0;
            let mut acc_dy: i32 = 0;
            let mut acc_dz: i32 = 0;
            let flush_rel = |desk: &mut DesktopShell, dx: &mut i32, dy: &mut i32, dz: &mut i32| {
                if *dx != 0 {
                    let mut ev = syscall::InputEventWire::default();
                    ev.device = syscall::INPUT_DEVICE_MOUSE;
                    ev.code = 0x0110;
                    ev.value = (*dx).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
                    desk.on_input_event(ev);
                    *dx = 0;
                }
                if *dy != 0 {
                    let mut ev = syscall::InputEventWire::default();
                    ev.device = syscall::INPUT_DEVICE_MOUSE;
                    ev.code = 0x0111;
                    ev.value = (*dy).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
                    desk.on_input_event(ev);
                    *dy = 0;
                }
                if *dz != 0 {
                    let mut ev = syscall::InputEventWire::default();
                    ev.device = syscall::INPUT_DEVICE_MOUSE;
                    ev.code = 0x0112;
                    ev.value = (*dz).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
                    desk.on_input_event(ev);
                    *dz = 0;
                }
            };
            let mut pending = first_ev;
            for _ in 0..256 {
                let Some(ev) = pending.take().or_else(|| recv_input_event_nb(iep)) else { break; };
                if ev.device == syscall::INPUT_DEVICE_MOUSE && ev.code == 0x0110 {
                    acc_dx = acc_dx.saturating_add(ev.value as i32);
                } else if ev.device == syscall::INPUT_DEVICE_MOUSE && ev.code == 0x0111 {
                    acc_dy = acc_dy.saturating_add(ev.value as i32);
                } else if ev.device == syscall::INPUT_DEVICE_MOUSE && ev.code == 0x0112 {
                    acc_dz = acc_dz.saturating_add(ev.value as i32);
                } else {
                    flush_rel(desk, &mut acc_dx, &mut acc_dy, &mut acc_dz);
                    desk.on_input_event(ev);
                }
            }
            flush_rel(desk, &mut acc_dx, &mut acc_dy, &mut acc_dz);
            if desk.mouse_x != prev_mx || desk.mouse_y != prev_my {
                sync_cursor_to_fb_server(pid, desk.mouse_x, desk.mouse_y);
            }
            let want_grab = desk.needs_kbd_grab();
            if want_grab != kbd_grabbed {
                set_keyboard_grab(pid, iep, want_grab);
                kbd_grabbed = want_grab;
            }
        }

        if let Some(desk) = desktop.as_mut() {
            desk.tick_frame();
            let now_ns = bench::now_ns();
            if now_ns.saturating_sub(last_periodic_ns) >= 500_000_000 {
                last_periodic_ns = now_ns;
                desk.tick_periodic();
            }
        }

        if let (Some(grant), Some(desk)) = (fb_grant.as_mut(), desktop.as_mut()) {
            let now_ns = bench::now_ns();
            let can_present = !presented_initial
                || now_ns.saturating_sub(last_present_ns) >= TARGET_FRAME_NS;
            if can_present {
                let dirty_rects = desk.take_dirty_rects();
                if !dirty_rects.is_empty() {
                    let (gw, gh) = (grant.width, grant.height);
                    let t0 = bench::now_ns();
                    // 1. Render all dirty rectangles into backbuf -> SHM before submitting to fb_server
                    if let Some(pixels) = grant.pixels_mut() {
                        for dirty_r in &dirty_rects {
                            desk.render_rect_to_shm(pixels, *dirty_r);
                        }
                    }
                    // 2. Pipeline intermediate rects (reply_ep = 0) and wait for ONE single post-flush ACK
                    //    on the final rect so fb_server processes the whole frame in one atomic batch!
                    let last_idx = dirty_rects.len() - 1;
                    for (i, dirty_r) in dirty_rects.iter().enumerate() {
                        let dmg = syscall::DamageRect {
                            x: dirty_r.x.max(0) as u32,
                            y: dirty_r.y.max(0) as u32,
                            width: (dirty_r.w.max(0) as u32).min(gw),
                            height: (dirty_r.h.max(0) as u32).min(gh),
                        };
                        let ack_ep = if i == last_idx { ep } else { 0 };
                        submit_damage_to_fb_server(pid, ack_ep, grant, dmg);
                    }
                    let frame_us = bench::now_ns().saturating_sub(t0) / 1000;
                    // Smooth 4-frame EMA so single cold-start repaints don't freeze the HUD at a stale spike
                    desk.last_render_us = if desk.last_render_us == 0 {
                        frame_us
                    } else {
                        (desk.last_render_us * 3 + frame_us) / 4
                    };
                    desk.on_frame_presented();
                    last_present_ns = bench::now_ns();
                    fps_frames = fps_frames.saturating_add(1);
                    let win_dt = last_present_ns.saturating_sub(fps_window_start_ns);
                    let est = if desk.last_render_us > 0 {
                        (1_000_000 / desk.last_render_us).clamp(1, 60) as u32
                    } else {
                        60
                    };
                    if win_dt >= 1_000_000_000 {
                        desk.measured_fps = ((fps_frames as u64 * 1_000_000_000) / win_dt.max(1))
                            .max(est as u64)
                            .clamp(1, 60) as u32;
                        fps_frames = 0;
                        fps_window_start_ns = last_present_ns;
                    } else {
                        desk.measured_fps = est;
                    }
                    if !presented_initial {
                        presented_initial = true;
                        syscall::boot_log(b"exo_compositor: Desktop initial frame presented\n");
                    }
                }
            }
            if let Some(req) = desk.take_power_request() {
                request_init_power_action(pid, matches!(req, PowerRequest::Reboot));
            }
        }

        if epfd >= 0 {
            let _ = unsafe {
                syscall::syscall4(syscall::SYS_EPOLL_WAIT, epfd as u64, events.as_mut_ptr() as u64, events.len() as u64, 0)
            };
        }
        accept_and_poll_wayland(listen_fd, epfd, pid, fb_grant.as_ref(), &mut clients);
    }
}

fn accept_and_poll_wayland(
    listen_fd: i32,
    epfd: i32,
    pid: u32,
    fb_grant: Option<&fb_grant_client::FbSurfaceGrant>,
    clients: &mut Vec<ClientState>,
) {
    loop {
        let cfd = unsafe { syscall::syscall3(syscall::SYS_ACCEPT, listen_fd as u64, 0, 0) };
        if cfd < 0 { break; }
        if clients.len() >= MAX_WAYLAND_CLIENTS {
            unsafe { let _ = syscall::syscall1(syscall::SYS_CLOSE, cfd as u64); }
            continue;
        }
        if epfd >= 0 {
            let ev = EpollEvent { events: EPOLLIN, data: cfd as u64 };
            let _ = unsafe { syscall::syscall4(syscall::SYS_EPOLL_CTL, epfd as u64, EPOLL_CTL_ADD, cfd as u64, &ev as *const EpollEvent as u64) };
        }
        clients.push(ClientState::new(cfd as i32));
    }

    let mut idx = 0usize;
    while idx < clients.len() {
        let mut disconnected = false;
        {
            let client = &mut clients[idx];
            let mut tmp = [0u8; 2048];
            for _ in 0..4 {
                if client.rx_buf.len() >= MAX_RX_BUF { disconnected = true; break; }
                let mut iov = RawIoVec { iov_base: tmp.as_mut_ptr() as u64, iov_len: tmp.len() as u64 };
                let mut ctrl = [0u8; 128];
                let mut msg = UnixMsghdr {
                    msg_name: 0, msg_namelen: 0, _pad0: 0,
                    msg_iov: &mut iov as *mut RawIoVec as u64, msg_iovlen: 1,
                    msg_control: ctrl.as_mut_ptr() as u64, msg_controllen: ctrl.len() as u64,
                    msg_flags: 0, _pad1: 0,
                };
                let rc = unsafe { syscall::syscall3(syscall::SYS_RECVMSG, client.fd as u64, &mut msg as *mut UnixMsghdr as u64, MSG_DONTWAIT) };
                if rc < 0 {
                    if rc != syscall::EAGAIN && rc != EINTR_ERR { disconnected = true; }
                    break;
                }
                if msg.msg_controllen >= core::mem::size_of::<UnixCmsghdr>() as u64 {
                    let cmsg = unsafe { core::ptr::read_unaligned(ctrl.as_ptr() as *const UnixCmsghdr) };
                    if cmsg.cmsg_level == 1 && cmsg.cmsg_type == 1 && cmsg.cmsg_len >= 16 {
                        let avail = (msg.msg_controllen as usize).min(ctrl.len()).min(cmsg.cmsg_len);
                        for fi in 0..(avail.saturating_sub(16) / 4) {
                            let off = 16 + fi * 4;
                            let rfd = i32::from_le_bytes([ctrl[off], ctrl[off + 1], ctrl[off + 2], ctrl[off + 3]]);
                            if client.rx_fds.len() < 16 { client.rx_fds.push_back(rfd); }
                            else if rfd >= 0 { unsafe { let _ = syscall::syscall1(syscall::SYS_CLOSE, rfd as u64); } }
                        }
                    }
                }
                if rc == 0 && msg.msg_controllen == 0 { disconnected = true; break; }
                client.rx_buf.extend_from_slice(&tmp[..rc as usize]);
            }

            if !disconnected && client.rx_buf.len() >= 8 && !dispatch_client_messages(client, pid, fb_grant) {
                disconnected = true;
            }

            if !disconnected && !client.tx_buf.is_empty() {
                let iov = RawIoVec { iov_base: client.tx_buf.as_ptr() as u64, iov_len: client.tx_buf.len() as u64 };
                let msg = UnixMsghdr {
                    msg_name: 0, msg_namelen: 0, _pad0: 0,
                    msg_iov: &iov as *const RawIoVec as u64, msg_iovlen: 1,
                    msg_control: 0, msg_controllen: 0, msg_flags: 0, _pad1: 0,
                };
                let sent = unsafe { syscall::syscall3(syscall::SYS_SENDMSG, client.fd as u64, &msg as *const UnixMsghdr as u64, MSG_DONTWAIT) };
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
                let _ = unsafe { syscall::syscall4(syscall::SYS_EPOLL_CTL, epfd as u64, EPOLL_CTL_DEL, cfd as u64, 0) };
            }
            let mut removed = clients.swap_remove(idx);
            removed.cleanup();
        } else {
            idx += 1;
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
