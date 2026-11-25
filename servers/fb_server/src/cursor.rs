//! 12x18 software/hardware cursor sprite and pointer interaction state for `fb_server`.

use crate::desktop_mut;
use crate::framebuffer::{notify_virtio_gpu_move_cursor, Framebuffer};

pub const MOUSE_CURSOR_W: u32 = 12;
pub const MOUSE_CURSOR_H: u32 = 18;

static MOUSE_CURSOR_BITMAP: [[u8; 12]; 18] = [
    [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 2, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 2, 3, 2, 1, 0, 0, 0, 0, 0, 0, 0],
    [1, 2, 3, 3, 2, 1, 0, 0, 0, 0, 0, 0],
    [1, 2, 3, 3, 3, 2, 1, 0, 0, 0, 0, 0],
    [1, 2, 3, 3, 3, 3, 2, 1, 0, 0, 0, 0],
    [1, 2, 3, 3, 3, 3, 3, 2, 1, 0, 0, 0],
    [1, 2, 2, 2, 2, 2, 2, 2, 2, 1, 0, 0],
    [1, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 0],
    [1, 2, 2, 1, 2, 3, 1, 0, 0, 0, 0, 0],
    [1, 2, 1, 0, 1, 2, 3, 1, 0, 0, 0, 0],
    [1, 1, 0, 0, 1, 2, 3, 1, 0, 0, 0, 0],
    [1, 0, 0, 0, 0, 1, 2, 2, 1, 0, 0, 0],
    [0, 0, 0, 0, 0, 1, 2, 2, 1, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
];

pub struct MouseState {
    pub x: u32,
    pub y: u32,
    pub visible: bool,
    pub drawn: bool,
    saved_pixels: [u32; 16 * 20],
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
            saved_pixels: [0; 16 * 20],
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

    pub fn restore_background(&mut self, fb: Framebuffer) {
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
        fb.flush_rect(self.saved_x, self.saved_y, self.saved_w, self.saved_h);
        self.drawn = false;
    }

    pub fn draw_cursor(&mut self, fb: Framebuffer) {
        if !self.visible || !fb.is_present() {
            return;
        }
        if self.drawn {
            self.restore_background(fb);
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
        let black = fb.encode(0, 0, 0);
        let white = fb.encode(255, 255, 255);
        let cyan = fb.encode(0x00, 0xc5, 0xff);
        cy = 0;
        while cy < h {
            let row_offset = ((y + cy) as usize).saturating_mul(row_stride);
            let mut cx = 0u32;
            while cx < w {
                let pixel = match MOUSE_CURSOR_BITMAP[cy as usize][cx as usize] {
                    1 => Some(black),
                    2 => Some(white),
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
        fb.flush_rect(x, y, w, h);
        notify_virtio_gpu_move_cursor(x, y);
        self.drawn = true;
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
