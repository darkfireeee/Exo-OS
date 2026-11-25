#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![allow(dead_code, unused_imports, unused_variables, unused_mut)]

//! # `fb_server` — Strate 1 Framebuffer MMIO Guardian & Emergency Display Server (S5B)
//!
//! Modular Strate 1 layout:
//! - `framebuffer.rs`: MMIO + shadow RAM backbuffer + damage flush + VirtIO-GPU bridge.
//! - `vt_console.rs`: ANSI VT100 emergency/boot terminal console.
//! - `cursor.rs`: 12x18 cursor sprite & pointer interaction state.
//! - `surface_table.rs`: Typed `SurfaceGrant` / `DamageRect` scanout table.
//! - `desktop.rs`: Strate 1 fallback desktop shell & RTC/telemetry HUD.
//!
//! Full Wayland wire & `/run/wayland-0` socket serving live exclusively in Strate 2
//! (`servers/exo_compositor`).

#[cfg(target_os = "none")]
use core::cell::UnsafeCell;
#[cfg(target_os = "none")]
use exo_syscall_abi as syscall;

#[cfg(target_os = "none")]
pub use exo_font as shared_font;
#[cfg(target_os = "none")]
mod cursor;
#[cfg(target_os = "none")]
mod desktop;
#[cfg(target_os = "none")]
mod framebuffer;
#[cfg(target_os = "none")]
mod surface_table;
#[cfg(target_os = "none")]
mod vt_console;

#[cfg(target_os = "none")]
pub use framebuffer::Framebuffer;

#[cfg(target_os = "none")]
use cursor::MouseState;
#[cfg(target_os = "none")]
use desktop::{DesktopState, DockOverlay, DOCK_BAR_H, KickoffAction};
#[cfg(target_os = "none")]
use surface_table::{surface_table_mut, UiAction};
#[cfg(target_os = "none")]
use vt_console::console_mut;

// Doit rester PAIR car bit 0 = IPC_FLAG_TIMEOUT (0x0001).
#[cfg(target_os = "none")]
const RECV_TIMEOUT_MS: u64 = 16;
#[cfg(target_os = "none")]
const BATCH_DRAIN_MAX: usize = 64;

#[cfg(target_os = "none")]
struct DesktopCell(UnsafeCell<DesktopState>);
#[cfg(target_os = "none")]
unsafe impl Sync for DesktopCell {}
#[cfg(target_os = "none")]
static DESKTOP: DesktopCell = DesktopCell(UnsafeCell::new(DesktopState::new()));

#[cfg(target_os = "none")]
pub(crate) fn desktop_mut() -> &'static mut DesktopState {
    unsafe { &mut *DESKTOP.0.get() }
}

#[cfg(target_os = "none")]
fn exit_failed() -> ! {
    unsafe {
        let _ = syscall::syscall1(syscall::SYS_EXIT, 127);
        let _ = syscall::syscall1(syscall::SYS_EXIT_GROUP, 127);
    }
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(target_os = "none")]
fn map_framebuffer() -> bool {
    let mut info = syscall::FramebufferInfoWire::default();
    let info_rc = unsafe {
        syscall::syscall1(
            syscall::SYS_FRAMEBUFFER_INFO,
            &mut info as *mut syscall::FramebufferInfoWire as u64,
        )
    };
    if info_rc < 0
        || info.phys_addr == 0
        || info.width == 0
        || info.height == 0
        || info.size_bytes == 0
    {
        syscall::boot_log(b"fb_server: framebuffer_info invalid\n");
        return false;
    }
    let virt = unsafe { syscall::syscall2(syscall::SYS_MMIO_MAP, info.phys_addr, info.size_bytes) };
    if virt <= 0 {
        syscall::boot_log(b"fb_server: mmio_map failed\n");
        return false;
    }
    let shadow_rc = unsafe {
        syscall::syscall6(
            syscall::SYS_MMAP,
            0,
            info.size_bytes,
            3,    // PROT_READ | PROT_WRITE
            0x22, // MAP_PRIVATE | MAP_ANONYMOUS
            u64::MAX,
            0,
        )
    };
    let shadow_addr = if shadow_rc > 0 {
        syscall::boot_log(b"fb_server: shadow buffer mapped\n");
        shadow_rc as u64
    } else {
        syscall::boot_log(b"fb_server: shadow buffer unavailable, using direct mmio\n");
        0
    };
    console_mut().configure(Framebuffer {
        virt_addr: virt as u64,
        shadow_addr,
        width: info.width,
        height: info.height,
        stride_pixels: info.stride_pixels,
        bpp: info.bpp,
        format: info.format,
        size_bytes: info.size_bytes,
    });
    true
}

#[cfg(target_os = "none")]
fn send_fb_reply(endpoint: u64, reply: &syscall::FbReply) {
    let _ = syscall::send_reply(endpoint, reply);
}

#[cfg(target_os = "none")]
fn handle_in_batch(req: &syscall::FbRequest) -> syscall::FbReply {
    match req.msg_type {
        syscall::FB_MSG_WRITE => {
            let n = core::cmp::min(req.a as usize, syscall::FB_TEXT_MAX);
            console_mut().write_all_no_cursor(&req.data[..n]);
            syscall::FbReply {
                status: 0,
                len: n as u32,
                _pad: 0,
            }
        }
        syscall::FB_MSG_CLEAR => {
            console_mut().clear();
            syscall::FbReply::default()
        }
        syscall::FB_MSG_SCROLL => {
            console_mut().scroll_lines(req.a as u32);
            syscall::FbReply::default()
        }
        syscall::FB_MSG_SET_CURSOR => {
            let console = console_mut();
            if console.cursor_drawn {
                console.draw_cursor(false);
            }
            console.col = (req.a as u32).min(console.cols.saturating_sub(1));
            console.row = ((req.b & 0xffff_ffff) as u32).min(console.rows.saturating_sub(1));
            console.cursor_visible = req.b & (1u64 << 63) != 0;
            syscall::FbReply::default()
        }
        syscall::DISPLAY_OP_GET_INFO => {
            let fb = console_mut().fb;
            syscall::FbReply {
                status: ((fb.width as i64) << 32) | (fb.height as i64),
                len: fb.bpp,
                _pad: fb.format,
            }
        }
        syscall::DISPLAY_OP_CREATE_SURFACE => {
            let w = (req.a & 0xFFFF_FFFF) as u32;
            let h = (req.b & 0xFFFF_FFFF) as u32;
            if let Some(idx) =
                surface_table_mut().alloc_surface(req.sender_pid, w.max(64), h.max(64))
            {
                let s = surface_table_mut().surfaces[idx];
                let return_addr = if s.shm_handle != 0 {
                    s.shm_handle
                } else {
                    s.virt_addr
                };
                syscall::FbReply {
                    status: return_addr as i64,
                    len: s.surface_id,
                    _pad: s.stride_bytes,
                }
            } else {
                syscall::FbReply {
                    status: syscall::ENOMEM,
                    len: 0,
                    _pad: 0,
                }
            }
        }
        syscall::DISPLAY_OP_DESTROY_SURFACE => {
            let ok = surface_table_mut().destroy_surface(req.a as u32, req.sender_pid);
            syscall::FbReply {
                status: if ok { 0 } else { syscall::ENOENT },
                len: 0,
                _pad: 0,
            }
        }
        syscall::DISPLAY_OP_COMMIT_DAMAGE | syscall::DISPLAY_OP_FLIP => {
            let target_surface_id = if (req.a >> 32) != 0 {
                (req.a >> 32) as u32
            } else {
                req.a as u32
            };
            let rect = syscall::DamageRect {
                x: (req.a & 0xFFFF) as u32,
                y: ((req.a >> 16) & 0xFFFF) as u32,
                width: (req.b & 0xFFFF) as u32,
                height: ((req.b >> 16) & 0xFFFF) as u32,
            };
            let fb = console_mut().fb;
            let wire_cnt = surface_table_mut().wire_messages_processed;
            let surf_cnt = surface_table_mut().active_surface_count();
            desktop_mut().set_wayland_stats(wire_cnt, surf_cnt);
            if let Some((y_min, y_max)) =
                surface_table_mut().commit_surface_damage(target_surface_id, req.sender_pid, rect, fb)
            {
                render_band_layers(fb, y_min, y_max);
                console_mut().mark_dirty_rows(y_min.min(fb.height), y_max.min(fb.height));
            } else {
                console_mut().mark_dirty_rows(0, fb.height);
            }
            syscall::FbReply::default()
        }
        syscall::DISPLAY_OP_WAYLAND_WIRE => {
            let raw_len = if req.a != 0 { req.a } else { req.b };
            let n = core::cmp::min(raw_len as usize, syscall::FB_TEXT_MAX);
            let count = surface_table_mut().acknowledge_terminal_wire(req.sender_pid, n);
            let wire_cnt = surface_table_mut().wire_messages_processed;
            let surf_cnt = surface_table_mut().active_surface_count();
            desktop_mut().set_wayland_stats(wire_cnt, surf_cnt);
            syscall::FbReply {
                status: count,
                len: n as u32,
                _pad: 0,
            }
        }
        _ => syscall::FbReply {
            status: syscall::EINVAL,
            len: 0,
            _pad: 0,
        },
    }
}

#[cfg(target_os = "none")]
fn register_input_stream() -> Option<u64> {
    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) };
    if pid <= 0 {
        return None;
    }
    let endpoint = ((pid as u64) << 32) | 1;
    let name = b"fb_server_input";
    let rc = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            endpoint,
        )
    };
    if rc < 0 && rc != syscall::EEXIST {
        return None;
    }
    let req = syscall::InputRequest {
        sender_pid: pid as u32,
        msg_type: syscall::INPUT_MSG_ATTACH,
        reply_endpoint: endpoint,
        event: syscall::InputEventWire::default(),
    };
    let send_rc = unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            syscall::INPUT_SERVER_ENDPOINT,
            &req as *const syscall::InputRequest as u64,
            core::mem::size_of::<syscall::InputRequest>() as u64,
            0,
            0,
            0,
        )
    };
    if send_rc < 0 {
        return None;
    }
    Some(endpoint)
}

#[cfg(target_os = "none")]
fn render_all_layers(fb: Framebuffer) {
    if !fb.is_present() {
        return;
    }
    desktop::render_desktop_base_with_wayland(fb, surface_table_mut());
    console_mut().redraw_all(fb);
    desktop_mut().render_active_overlay(fb);
    if surface_table_mut().kickoff_open && fb.height > DOCK_BAR_H + 260 && fb.width > 380 {
        desktop::render_kickoff_menu(
            fb,
            surface_table_mut().selected_category,
            surface_table_mut().selected_app,
        );
    }
    desktop::render_bottom_panel_with_wayland(fb, surface_table_mut());
}

#[cfg(target_os = "none")]
fn render_band_layers(fb: Framebuffer, y0: u32, y1: u32) {
    if !fb.is_present() {
        return;
    }
    let desktop_h = fb.height.saturating_sub(DOCK_BAR_H);
    let by0 = y0.min(desktop_h);
    let by1 = y1.min(desktop_h);
    if by0 < by1 {
        desktop::render_wallpaper_rect(fb, 0, by0, fb.width, by1 - by0);
    }
    surface_table_mut().composite_all_surfaces(fb);
    console_mut().redraw_all(fb);
    desktop_mut().render_active_overlay(fb);
    if surface_table_mut().kickoff_open && fb.height > DOCK_BAR_H + 260 && fb.width > 380 {
        desktop::render_kickoff_menu(
            fb,
            surface_table_mut().selected_category,
            surface_table_mut().selected_app,
        );
    }
    if y1 > desktop_h {
        desktop::render_bottom_panel_with_wayland(fb, surface_table_mut());
    }
}

#[cfg(target_os = "none")]
fn push_keyboard_bytes(bytes: &[u8]) {
    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) } as u32;
    for &b in bytes {
        let ev = syscall::InputEventWire {
            device: syscall::INPUT_DEVICE_KEYBOARD,
            state: syscall::INPUT_KEY_PRESSED,
            code: 0,
            value: 1,
            ascii: b,
            modifiers: 0,
            _pad: [0; 4],
        };
        let req = syscall::InputRequest {
            sender_pid: pid,
            msg_type: syscall::INPUT_MSG_PUSH,
            reply_endpoint: 0,
            event: ev,
        };
        let _ = unsafe {
            syscall::syscall6(
                syscall::SYS_IPC_SEND,
                syscall::INPUT_SERVER_ENDPOINT,
                &req as *const syscall::InputRequest as u64,
                core::mem::size_of::<syscall::InputRequest>() as u64,
                syscall::IPC_FLAG_TIMEOUT | 4,
                0,
                0,
            )
        };
    }
}

#[cfg(target_os = "none")]
fn compositor_repaint(mouse: &mut MouseState, fb: Framebuffer, y0: u32, y1: u32, full: bool) {
    if !fb.is_present() {
        return;
    }
    if mouse.drawn {
        mouse.restore_background(fb);
    }
    if full {
        render_all_layers(fb);
    } else {
        render_band_layers(fb, y0, y1);
    }
    console_mut().mark_dirty_rows(y0.min(fb.height), y1.min(fb.height));
    console_mut().flush_damage();
    mouse.draw_cursor(fb);
}

#[cfg(target_os = "none")]
fn execute_kickoff_action(action: KickoffAction, mouse: &mut MouseState, fb: Framebuffer) {
    match action {
        KickoffAction::OpenTerminal => {
            surface_table_mut().focus_surface(0);
            compositor_repaint(mouse, fb, 0, fb.height, true);
        }
        KickoffAction::RunCmd(cmd) => {
            surface_table_mut().focus_surface(0);
            compositor_repaint(mouse, fb, 0, fb.height, true);
            push_keyboard_bytes(cmd);
        }
        KickoffAction::ToggleOverlay(ov) => {
            desktop_mut().toggle_overlay(ov);
            compositor_repaint(mouse, fb, 0, fb.height, true);
        }
        KickoffAction::ClearTerminal => {
            surface_table_mut().focus_surface(0);
            compositor_repaint(mouse, fb, 0, fb.height, true);
            push_keyboard_bytes(b"clear\n");
        }
        KickoffAction::CloseMenu => {
            compositor_repaint(mouse, fb, 0, fb.height, true);
        }
        KickoffAction::Reboot => {
            surface_table_mut().focus_surface(0);
            compositor_repaint(mouse, fb, 0, fb.height, true);
            push_keyboard_bytes(b"reboot\n");
        }
        KickoffAction::Shutdown => {
            surface_table_mut().focus_surface(0);
            compositor_repaint(mouse, fb, 0, fb.height, true);
            push_keyboard_bytes(b"shutdown\n");
        }
    }
}

#[cfg(target_os = "none")]
fn drain_mouse_events(endpoint: u64, mouse: &mut MouseState, fb: Framebuffer) {
    let mut reply = syscall::InputReply::default();
    let mut drained = 0usize;
    let mut total_dx: i16 = 0;
    let mut total_dy: i16 = 0;
    while drained < 64 {
        let rc = unsafe {
            syscall::syscall4(
                syscall::SYS_EXO_IPC_RECV_NB,
                endpoint,
                &mut reply as *mut syscall::InputReply as u64,
                core::mem::size_of::<syscall::InputReply>() as u64,
                0,
            )
        };
        if rc < core::mem::size_of::<syscall::InputReply>() as i64 {
            break;
        }
        if reply.status == 0 && reply.event.device == syscall::INPUT_DEVICE_MOUSE {
            match reply.event.code {
                0x0110 => total_dx = total_dx.saturating_add(reply.event.value),
                0x0111 => total_dy = total_dy.saturating_add(reply.event.value),
                0 => {
                    total_dx = total_dx.saturating_add(reply.event.value);
                    total_dy = total_dy.saturating_add((reply.event.ascii as i8) as i16);
                }
                0x0100 | 1 => {
                    let pressed = reply.event.value != 0;
                    let was_pressed = mouse.left_button;
                    mouse.left_button = pressed;
                    if pressed && !was_pressed {
                        if total_dx != 0 || total_dy != 0 {
                            mouse.update_position(total_dx, total_dy, fb);
                            total_dx = 0;
                            total_dy = 0;
                        }
                        let (hud_hit, hud_copy) =
                            desktop_mut().handle_mouse_click_with_copy(mouse.x, mouse.y, fb);
                        if hud_hit {
                            if let Some(txt) = hud_copy {
                                mouse.set_clipboard(txt);
                            }
                            compositor_repaint(mouse, fb, 0, fb.height, true);
                        } else {
                            match surface_table_mut().handle_pointer_press(mouse.x, mouse.y, fb) {
                                UiAction::Redraw(y0, y1) => {
                                    compositor_repaint(mouse, fb, y0, y1, true);
                                }
                                UiAction::Kickoff(act) => {
                                    execute_kickoff_action(act, mouse, fb);
                                }
                                UiAction::StartDrag { surf_idx, off_x, off_y } => {
                                    mouse.drag_surface = Some((surf_idx, off_x, off_y));
                                    compositor_repaint(mouse, fb, 0, fb.height, true);
                                }
                                UiAction::StartResize {
                                    surf_idx,
                                    start_mx,
                                    start_my,
                                    orig_w,
                                    orig_h,
                                } => {
                                    mouse.resize_surface =
                                        Some((surf_idx, start_mx, start_my, orig_w, orig_h));
                                    compositor_repaint(mouse, fb, 0, fb.height, true);
                                }
                                UiAction::StartTermSelect { .. } => {
                                    let old_sel = console_mut().normalized_selection();
                                    console_mut().sel_start = None;
                                    console_mut().sel_end = None;
                                    if let Some((a, b)) = old_sel {
                                        if mouse.drawn {
                                            mouse.restore_background(fb);
                                        }
                                        console_mut().redraw_cell_rows(fb, a.1, b.1);
                                        console_mut().flush_damage();
                                        mouse.draw_cursor(fb);
                                    }
                                    if let Some(cell) = console_mut().pixel_to_cell(mouse.x, mouse.y) {
                                        mouse.selecting_text = true;
                                        console_mut().sel_start = Some(cell);
                                        console_mut().sel_end = Some(cell);
                                    }
                                }
                                UiAction::ClientPointer { .. } => {
                                    compositor_repaint(mouse, fb, 0, fb.height, true);
                                }
                                UiAction::None => {}
                            }
                        }
                    } else if !pressed && was_pressed {
                        if total_dx != 0 || total_dy != 0 {
                            mouse.update_position(total_dx, total_dy, fb);
                            total_dx = 0;
                            total_dy = 0;
                        }
                        if mouse.drag_surface.is_some() || mouse.resize_surface.is_some() {
                            mouse.drag_surface = None;
                            mouse.resize_surface = None;
                        }
                        if mouse.selecting_text {
                            mouse.selecting_text = false;
                            let prev_sel = console_mut().sel_end;
                            let cur = console_mut().pixel_to_cell_clamped(mouse.x, mouse.y);
                            if Some(cur) != prev_sel {
                                let anchor = console_mut().sel_start.unwrap_or(cur);
                                let old_c = prev_sel.unwrap_or(cur);
                                let r0 = anchor.1.min(old_c.1).min(cur.1);
                                let r1 = anchor.1.max(old_c.1).max(cur.1);
                                console_mut().sel_end = Some(cur);
                                if mouse.drawn {
                                    mouse.restore_background(fb);
                                }
                                console_mut().redraw_cell_rows(fb, r0, r1);
                                console_mut().flush_damage();
                                mouse.draw_cursor(fb);
                            }
                            if console_mut().has_active_selection() {
                                let mut clip_tmp = [0u8; 4096];
                                let n = console_mut().extract_selection(&mut clip_tmp);
                                mouse.set_clipboard(&clip_tmp[..n]);
                            }
                        }
                    }
                }
                0x0101 | 0x0102 => {
                    let pressed = reply.event.value != 0;
                    let is_right = reply.event.code == 0x0101;
                    let was = if is_right {
                        mouse.right_button
                    } else {
                        mouse.middle_button
                    };
                    if is_right {
                        mouse.right_button = pressed;
                    } else {
                        mouse.middle_button = pressed;
                    }
                    if pressed && !was && mouse.clipboard_len > 0 {
                        if console_mut().pixel_to_cell(mouse.x, mouse.y).is_some()
                            && !surface_table_mut().surfaces[0].minimized
                        {
                            surface_table_mut().focus_surface(0);
                            console_mut().sel_start = None;
                            console_mut().sel_end = None;
                            compositor_repaint(mouse, fb, 0, fb.height, true);
                            push_keyboard_bytes(&mouse.clipboard[..mouse.clipboard_len]);
                        }
                    }
                }
                _ => {}
            }
        }
        drained += 1;
    }

    if total_dx != 0 || total_dy != 0 {
        mouse.update_position(total_dx, total_dy, fb);

        if let Some((s_idx, off_x, off_y)) = mouse.drag_surface {
            if mouse.left_button {
                let old_bounds = surface_table_mut().surface_window_bounds(s_idx, fb);
                let target_x = mouse.x as i32 - off_x;
                let target_y = mouse.y as i32 - off_y;
                surface_table_mut().move_surface(s_idx, target_x, target_y, fb);
                let new_bounds = surface_table_mut().surface_window_bounds(s_idx, fb);
                let (mut y0, mut y1) = match (old_bounds, new_bounds) {
                    (Some((_, oy, _, oh)), Some((_, ny, _, nh))) => (
                        oy.min(ny),
                        oy.saturating_add(oh + 8).max(ny.saturating_add(nh + 8)),
                    ),
                    _ => (0, fb.height),
                };
                y1 = y1.min(fb.height);
                if y0 >= y1 {
                    y0 = 0;
                    y1 = fb.height;
                }
                compositor_repaint(mouse, fb, y0, y1, false);
            } else {
                mouse.drag_surface = None;
            }
        } else if let Some((s_idx, start_mx, start_my, orig_w, orig_h)) = mouse.resize_surface {
            if mouse.left_button {
                let old_bounds = surface_table_mut().surface_window_bounds(s_idx, fb);
                let dw = mouse.x as i32 - start_mx as i32;
                let dh = mouse.y as i32 - start_my as i32;
                let new_w = (orig_w as i32 + dw).max(320) as u32;
                let new_h = (orig_h as i32 + dh).max(180) as u32;
                surface_table_mut().resize_surface(s_idx, new_w, new_h, fb);
                let new_bounds = surface_table_mut().surface_window_bounds(s_idx, fb);
                let (y0, y1) = match (old_bounds, new_bounds) {
                    (Some((_, oy, _, oh)), Some((_, ny, _, nh))) => (
                        oy.min(ny),
                        oy.saturating_add(oh + 8)
                            .max(ny.saturating_add(nh + 8))
                            .min(fb.height),
                    ),
                    _ => (0, fb.height),
                };
                compositor_repaint(mouse, fb, y0, y1, false);
            } else {
                mouse.resize_surface = None;
            }
        } else if mouse.selecting_text && mouse.left_button {
            let cur = console_mut().pixel_to_cell_clamped(mouse.x, mouse.y);
            if Some(cur) != console_mut().sel_end {
                let anchor = console_mut().sel_start.unwrap_or(cur);
                let prev = console_mut().sel_end.unwrap_or(cur);
                let r0 = anchor.1.min(prev.1).min(cur.1);
                let r1 = anchor.1.max(prev.1).max(cur.1);
                console_mut().sel_end = Some(cur);
                if mouse.drawn {
                    mouse.restore_background(fb);
                }
                console_mut().redraw_cell_rows(fb, r0, r1);
                console_mut().flush_damage();
                mouse.draw_cursor(fb);
            }
        }
    }
}

#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn _start() -> ! {
    syscall::boot_log(b"fb_server: boot\n");
    let _ = map_framebuffer();
    let mut mouse = MouseState::new();
    if console_mut().fb.is_present() {
        mouse.x = console_mut().fb.width / 2;
        mouse.y = console_mut().fb.height / 2;
        mouse.draw_cursor(console_mut().fb);
    }
    if !syscall::register_endpoint(b"fb_server", syscall::FB_SERVER_ENDPOINT) {
        syscall::boot_log(b"fb_server: register failed\n");
        exit_failed();
    }
    syscall::boot_log(b"fb_server: registered\n");

    let mut input_endpoint = register_input_stream();
    let mut last_clock_min = desktop::current_rtc_minute();

    let mut req = syscall::FbRequest::zeroed();
    loop {
        if input_endpoint.is_none() {
            input_endpoint = register_input_stream();
        }
        if let Some(ep) = input_endpoint {
            drain_mouse_events(ep, &mut mouse, console_mut().fb);
        }
        let now_min = desktop::current_rtc_minute();
        if now_min != last_clock_min && console_mut().fb.is_present() {
            last_clock_min = now_min;
            let fb = console_mut().fb;
            let dock_y = fb.height.saturating_sub(DOCK_BAR_H);
            if mouse.drawn {
                mouse.restore_background(fb);
            }
            desktop::render_bottom_panel_with_wayland(fb, surface_table_mut());
            if desktop_mut().active_overlay == DockOverlay::Calendar {
                desktop_mut().render_active_overlay(fb);
                console_mut().mark_dirty_rows(0, fb.height);
            } else {
                console_mut().mark_dirty_rows(dock_y, fb.height);
            }
            console_mut().flush_damage();
            mouse.draw_cursor(fb);
        }
        let rc = unsafe {
            syscall::syscall4(
                syscall::SYS_IPC_RECV,
                syscall::FB_SERVER_ENDPOINT,
                &mut req as *mut syscall::FbRequest as u64,
                core::mem::size_of::<syscall::FbRequest>() as u64,
                syscall::IPC_FLAG_TIMEOUT | RECV_TIMEOUT_MS,
            )
        };
        if rc < 0 {
            continue;
        }

        if mouse.drawn {
            mouse.restore_background(console_mut().fb);
        }

        let reply = handle_in_batch(&req);
        send_fb_reply(req.reply_endpoint, &reply);

        let mut batched = 0usize;
        while batched < BATCH_DRAIN_MAX {
            let nb_rc = unsafe {
                syscall::syscall4(
                    syscall::SYS_EXO_IPC_RECV_NB,
                    syscall::FB_SERVER_ENDPOINT,
                    &mut req as *mut syscall::FbRequest as u64,
                    core::mem::size_of::<syscall::FbRequest>() as u64,
                    0,
                )
            };
            if nb_rc < 0 {
                break;
            }
            let r = handle_in_batch(&req);
            send_fb_reply(req.reply_endpoint, &r);
            batched += 1;
        }

        console_mut().draw_cursor(true);
        console_mut().flush_damage();

        if !mouse.drawn {
            mouse.draw_cursor(console_mut().fb);
        }
    }
}

#[cfg(not(target_os = "none"))]
fn main() {}

#[cfg(target_os = "none")]
exo_syscall_abi::ring1_panic_handler!(b"fb_server: PANIC!\n");
