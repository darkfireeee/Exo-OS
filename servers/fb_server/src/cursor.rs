//! 16x22 crisp obsidian-and-crystal pointer sprite and interaction state for `fb_server`.

use crate::desktop_mut;
use crate::framebuffer::{notify_virtio_gpu_move_cursor, Framebuffer};

pub const MOUSE_CURSOR_W: u32 = 16;
pub const MOUSE_CURSOR_H: u32 = 22;

// 0 = transparent, 1 = crisp white outer edge, 2 = deep obsidian core, 3 = luminous ice-cyan crystal highlight
static MOUSE_CURSOR_BITMAP: [[u8; 16]; 22] = [
    [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 3, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 2, 3, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 2, 2, 3, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 2, 2, 2, 3, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 2, 2, 2, 2, 3, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 2, 2, 2, 2, 2, 3, 1, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 2, 2, 2, 2, 2, 2, 3, 1, 0, 0, 0, 0, 0, 0, 0],
    [1, 2, 2, 2, 2, 2, 2, 2, 3, 1, 0, 0, 0, 0, 0, 0],
    [1, 2, 2, 2, 2, 2, 2, 2, 2, 3, 1, 0, 0, 0, 0, 0],
    [1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3, 1, 0, 0, 0, 0],
    [1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3, 1, 0, 0, 0],
    [1, 2, 2, 2, 2, 2, 3, 3, 1, 1, 1, 1, 1, 1, 0, 0],
    [1, 2, 2, 2, 3, 2, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 2, 2, 1, 1, 3, 2, 2, 1, 0, 0, 0, 0, 0, 0, 0],
    [1, 2, 1, 0, 0, 1, 2, 2, 1, 0, 0, 0, 0, 0, 0, 0],
    [1, 1, 0, 0, 0, 1, 3, 2, 2, 1, 0, 0, 0, 0, 0, 0],
    [1, 0, 0, 0, 0, 0, 1, 2, 2, 1, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 1, 3, 2, 2, 1, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
];

pub struct MouseState {
    pub x: u32,
    pub y: u32,
    pub visible: bool,
    pub drawn: bool,
    saved_pixels: [u32; 16 * 22],
    saved_x: u32,
    saved_y: u32,
    saved_w: u32,
    saved_h: u32,
    pub left_button: bool,
    pub right_button: bool,
    pub middle_button: bool,
    pub drag_surface: Option<(usize, i32, i32)>,
    pub resize_surface: Option<(usize, u32, u32, u32, u32)>,
    pub selecting_text: bool,
    pub clipboard: [u8; 4096],
    pub clipboard_len: usize,
}

impl MouseState {
    pub const fn new() -> Self {
        Self {
            x: 512,
            y: 384,
            visible: true,
            drawn: false,
            saved_pixels: [0; 16 * 22],
            saved_x: 0,
            saved_y: 0,
            saved_w: 0,
            saved_h: 0,
            left_button: false,
            right_button: false,
            middle_button: false,
            drag_surface: None,
            resize_surface: None,
            selecting_text: false,
            clipboard: [0; 4096],
            clipboard_len: 0,
        }
    }

    pub fn set_clipboard(&mut self, bytes: &[u8]) {
        let n = bytes.len().min(self.clipboard.len());
        self.clipboard[..n].copy_from_slice(&bytes[..n]);
        self.clipboard_len = n;
        desktop_mut().set_clipboard_count(n);
    }

    #[inline]
    pub fn saved_rect(&self) -> Option<(u32, u32, u32, u32)> {
        if self.drawn && self.saved_w > 0 && self.saved_h > 0 {
            Some((self.saved_x, self.saved_y, self.saved_w, self.saved_h))
        } else {
            None
        }
    }

    /// Restore underlying cursor pixels into RAM shadow buffer ONLY (zero MMIO write -> zero flicker).
    pub fn restore_background_shadow_only(&mut self, fb: Framebuffer) {
        if !self.drawn || !fb.is_present() {
            return;
        }
        let w = self.saved_w;
        let h = self.saved_h;
        let bpp = fb.bytes_per_pixel() as usize;
        let target_base = fb.draw_target_base();
        let row_stride = (fb.stride_pixels as usize).saturating_mul(bpp);
        let mut cy = 0u32;
        while cy < h {
            let row_offset = ((self.saved_y + cy) as usize).saturating_mul(row_stride);
            let mut cx = 0u32;
            while cx < w {
                let pixel = self.saved_pixels[(cy * w + cx) as usize];
                let px_offset =
                    row_offset.saturating_add(((self.saved_x + cx) as usize).saturating_mul(bpp));
                let addr = target_base.saturating_add(px_offset);
                unsafe {
                    match bpp {
                        4 => core::ptr::write(addr as *mut u32, pixel),
                        3 => {
                            let bytes = pixel.to_le_bytes();
                            core::ptr::write(addr as *mut u8, bytes[0]);
                            core::ptr::write((addr + 1) as *mut u8, bytes[1]);
                            core::ptr::write((addr + 2) as *mut u8, bytes[2]);
                        }
                        _ => {}
                    }
                }
                cx += 1;
            }
            cy += 1;
        }
        self.drawn = false;
    }

    pub fn restore_background(&mut self, fb: Framebuffer) {
        if !self.drawn || !fb.is_present() {
            return;
        }
        let (sx, sy, sw, sh) = (self.saved_x, self.saved_y, self.saved_w, self.saved_h);
        self.restore_background_shadow_only(fb);
        fb.flush_rect(sx, sy, sw, sh);
    }

    /// Composite pointer sprite into RAM shadow buffer ONLY (returns drawn rect for atomic flush).
    pub fn draw_cursor_shadow_only(&mut self, fb: Framebuffer) -> Option<(u32, u32, u32, u32)> {
        if !self.visible || !fb.is_present() {
            return None;
        }
        if self.drawn {
            self.restore_background_shadow_only(fb);
        }
        let x = self.x.min(fb.width.saturating_sub(1));
        let y = self.y.min(fb.height.saturating_sub(1));
        let w = MOUSE_CURSOR_W.min(fb.width.saturating_sub(x));
        let h = MOUSE_CURSOR_H.min(fb.height.saturating_sub(y));
        let bpp = fb.bytes_per_pixel() as usize;
        let target_base = fb.draw_target_base();
        let row_stride = (fb.stride_pixels as usize).saturating_mul(bpp);
        let mut cy = 0u32;
        while cy < h {
            let mut cx = 0u32;
            while cx < w {
                self.saved_pixels[(cy * w + cx) as usize] = fb.read_pixel(x + cx, y + cy);
                cx += 1;
            }
            cy += 1;
        }
        self.saved_x = x;
        self.saved_y = y;
        self.saved_w = w;
        self.saved_h = h;
        let white = fb.encode(0xF8, 0xFC, 0xFF);
        let obsidian = fb.encode(0x09, 0x13, 0x28);
        let cyan = fb.encode(0x38, 0xBD, 0xF8);
        cy = 0;
        while cy < h {
            let row_offset = ((y + cy) as usize).saturating_mul(row_stride);
            let mut cx = 0u32;
            while cx < w {
                let pixel = match MOUSE_CURSOR_BITMAP[cy as usize][cx as usize] {
                    1 => Some(white),
                    2 => Some(obsidian),
                    3 => Some(cyan),
                    _ => None,
                };
                if let Some(p) = pixel {
                    let px_offset =
                        row_offset.saturating_add(((x + cx) as usize).saturating_mul(bpp));
                    let addr = target_base.saturating_add(px_offset);
                    unsafe {
                        match bpp {
                            4 => core::ptr::write(addr as *mut u32, p),
                            3 => {
                                let bytes = p.to_le_bytes();
                                core::ptr::write(addr as *mut u8, bytes[0]);
                                core::ptr::write((addr + 1) as *mut u8, bytes[1]);
                                core::ptr::write((addr + 2) as *mut u8, bytes[2]);
                            }
                            _ => {}
                        }
                    }
                }
                cx += 1;
            }
            cy += 1;
        }
        self.drawn = true;
        Some((x, y, w, h))
    }

    pub fn draw_cursor(&mut self, fb: Framebuffer) {
        if let Some((x, y, w, h)) = self.draw_cursor_shadow_only(fb) {
            fb.flush_rect(x, y, w, h);
            notify_virtio_gpu_move_cursor(x, y);
        }
    }

    pub fn set_position(&mut self, x: u32, y: u32, fb: Framebuffer) {
        if !fb.is_present() {
            return;
        }
        let nx = x.min(fb.width.saturating_sub(1));
        let ny = y.min(fb.height.saturating_sub(1));
        if nx == self.x && ny == self.y && self.drawn {
            return;
        }
        self.restore_background(fb);
        self.x = nx;
        self.y = ny;
        self.draw_cursor(fb);
    }

    pub fn update_position(&mut self, dx: i16, dy: i16, fb: Framebuffer) {
        let max_x = fb.width.saturating_sub(1) as i32;
        let max_y = fb.height.saturating_sub(1) as i32;
        let nx = (self.x as i32 + dx as i32).clamp(0, max_x) as u32;
        let ny = (self.y as i32 + dy as i32).clamp(0, max_y) as u32;
        if nx == self.x && ny == self.y {
            return;
        }
        self.restore_background(fb);
        self.x = nx;
        self.y = ny;
        self.draw_cursor(fb);
    }
}
