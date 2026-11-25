//! Strate 1 framebuffer MMIO guardian, RAM shadow buffer, and VirtIO-GPU flush bridge.

use exo_syscall_abi as syscall;

#[inline]
pub fn fill_buffer_rect(
    base: usize,
    row_stride: usize,
    x: u32,
    y: u32,
    x_end: u32,
    y_end: u32,
    bpp: usize,
    pixel: u32,
) {
    if base == 0 || x >= x_end || y >= y_end {
        return;
    }
    if bpp == 4 {
        let width = (x_end - x) as usize;
        let pixel64 = (pixel as u64) | ((pixel as u64) << 32);
        let mut row = y;
        while row < y_end {
            let row_base = base
                .saturating_add((row as usize).saturating_mul(row_stride))
                .saturating_add((x as usize).saturating_mul(4));
            let mut col = 0usize;
            while col < width {
                let addr = row_base + col * 4;
                unsafe {
                    if col + 1 < width && (addr & 7) == 0 {
                        core::ptr::write(addr as *mut u64, pixel64);
                        col += 2;
                    } else {
                        core::ptr::write(addr as *mut u32, pixel);
                        col += 1;
                    }
                }
            }
            row = row.saturating_add(1);
        }
        return;
    }
    if bpp == 3 {
        let bytes = pixel.to_le_bytes();
        let width = (x_end - x) as usize;
        let mut row = y;
        while row < y_end {
            let row_base = base
                .saturating_add((row as usize).saturating_mul(row_stride))
                .saturating_add((x as usize).saturating_mul(3));
            let mut col = 0usize;
            while col < width {
                let addr = row_base.saturating_add(col.saturating_mul(3));
                unsafe {
                    core::ptr::write(addr as *mut u8, bytes[0]);
                    core::ptr::write((addr + 1) as *mut u8, bytes[1]);
                    core::ptr::write((addr + 2) as *mut u8, bytes[2]);
                }
                col += 1;
            }
            row = row.saturating_add(1);
        }
    }
}

#[derive(Clone, Copy)]
pub struct Framebuffer {
    pub virt_addr: u64,
    pub shadow_addr: u64,
    pub width: u32,
    pub height: u32,
    pub stride_pixels: u32,
    pub bpp: u32,
    pub format: u32,
    pub size_bytes: u64,
}

impl Framebuffer {
    pub const fn absent() -> Self {
        Self {
            virt_addr: 0,
            shadow_addr: 0,
            width: 0,
            height: 0,
            stride_pixels: 0,
            bpp: 0,
            format: 0,
            size_bytes: 0,
        }
    }

    #[inline]
    pub fn is_present(self) -> bool {
        self.virt_addr != 0
            && self.width != 0
            && self.height != 0
            && self.stride_pixels >= self.width
            && self.size_bytes != 0
            && self.bytes_per_pixel() >= 3
    }

    #[inline]
    pub fn draw_target_base(self) -> usize {
        if self.shadow_addr != 0 {
            self.shadow_addr as usize
        } else {
            self.virt_addr as usize
        }
    }

    #[inline]
    pub fn bytes_per_pixel(self) -> u32 {
        ((self.bpp.saturating_add(7)) / 8).max(1)
    }

    #[inline]
    pub fn encode(self, r: u8, g: u8, b: u8) -> u32 {
        match self.format {
            1 => ((b as u32) << 16) | ((g as u32) << 8) | (r as u32),
            _ => ((r as u32) << 16) | ((g as u32) << 8) | (b as u32),
        }
    }

    #[inline]
    pub fn pixel_offset(self, x: u32, y: u32) -> Option<usize> {
        if !self.is_present() || x >= self.width || y >= self.height {
            return None;
        }
        let bpp = self.bytes_per_pixel() as usize;
        let offset = (y as usize)
            .checked_mul(self.stride_pixels as usize)?
            .checked_add(x as usize)?
            .checked_mul(bpp)?;
        if offset.checked_add(bpp)? <= self.size_bytes as usize {
            Some(offset)
        } else {
            None
        }
    }

    #[inline]
    pub fn put_pixel_direct(self, x: u32, y: u32, pixel: u32) {
        let Some(offset) = self.pixel_offset(x, y) else {
            return;
        };
        let bpp = self.bytes_per_pixel();
        let bytes = pixel.to_le_bytes();
        if self.shadow_addr != 0 {
            let s_addr = (self.shadow_addr as usize).saturating_add(offset);
            unsafe {
                match bpp {
                    4 => core::ptr::write(s_addr as *mut u32, pixel),
                    3 => {
                        core::ptr::write(s_addr as *mut u8, bytes[0]);
                        core::ptr::write((s_addr + 1) as *mut u8, bytes[1]);
                        core::ptr::write((s_addr + 2) as *mut u8, bytes[2]);
                    }
                    _ => {}
                }
            }
        }
        let addr = (self.virt_addr as usize).saturating_add(offset);
        unsafe {
            match bpp {
                4 => core::ptr::write_volatile(addr as *mut u32, pixel),
                3 => {
                    core::ptr::write_volatile(addr as *mut u8, bytes[0]);
                    core::ptr::write_volatile((addr + 1) as *mut u8, bytes[1]);
                    core::ptr::write_volatile((addr + 2) as *mut u8, bytes[2]);
                }
                _ => {}
            }
        }
    }

    #[inline]
    pub fn read_pixel(self, x: u32, y: u32) -> u32 {
        let Some(offset) = self.pixel_offset(x, y) else {
            return 0;
        };
        let base = self.draw_target_base();
        let addr = base.saturating_add(offset);
        unsafe {
            match self.bytes_per_pixel() {
                4 => core::ptr::read(addr as *const u32),
                3 => {
                    let b0 = core::ptr::read(addr as *const u8);
                    let b1 = core::ptr::read((addr + 1) as *const u8);
                    let b2 = core::ptr::read((addr + 2) as *const u8);
                    u32::from_le_bytes([b0, b1, b2, 0])
                }
                _ => 0,
            }
        }
    }

    #[inline]
    pub fn fill_rect_shadow(self, x: u32, y: u32, w: u32, h: u32, pixel: u32) {
        if !self.is_present() || w == 0 || h == 0 {
            return;
        }
        let x_end = x.saturating_add(w).min(self.width);
        let y_end = y.saturating_add(h).min(self.height);
        if x >= x_end || y >= y_end {
            return;
        }
        let bpp = self.bytes_per_pixel() as usize;
        let row_stride = (self.stride_pixels as usize).saturating_mul(bpp);
        fill_buffer_rect(
            self.draw_target_base(),
            row_stride,
            x,
            y,
            x_end,
            y_end,
            bpp,
            pixel,
        );
    }

    pub fn flush_rows(self, y_min: u32, y_max: u32) {
        if !self.is_present() || self.shadow_addr == 0 || y_min >= y_max {
            return;
        }
        let y0 = y_min.min(self.height) as usize;
        let y1 = y_max.min(self.height) as usize;
        if y0 >= y1 {
            return;
        }
        let bpp = self.bytes_per_pixel() as usize;
        let row_stride = (self.stride_pixels as usize).saturating_mul(bpp);
        let offset = y0.saturating_mul(row_stride);
        let len = (y1 - y0)
            .saturating_mul(row_stride)
            .min((self.size_bytes as usize).saturating_sub(offset));
        if len == 0 {
            return;
        }
        unsafe {
            let src = (self.shadow_addr as usize + offset) as *const u64;
            let dst = (self.virt_addr as usize + offset) as *mut u64;
            let qwords = len / 8;
            let mut i = 0usize;
            while i < qwords {
                core::ptr::write_volatile(dst.add(i), core::ptr::read(src.add(i)));
                i += 1;
            }
            let rem_start = qwords * 8;
            let src_b = (self.shadow_addr as usize + offset + rem_start) as *const u8;
            let dst_b = (self.virt_addr as usize + offset + rem_start) as *mut u8;
            let mut b = 0usize;
            while rem_start + b < len {
                core::ptr::write_volatile(dst_b.add(b), core::ptr::read(src_b.add(b)));
                b += 1;
            }
        }
    }

    pub fn flush_rect(self, x: u32, y: u32, w: u32, h: u32) {
        if !self.is_present() || self.shadow_addr == 0 || w == 0 || h == 0 {
            return;
        }
        let x0 = x.min(self.width) as usize;
        let y0 = y.min(self.height) as usize;
        let x1 = (x.saturating_add(w)).min(self.width) as usize;
        let y1 = (y.saturating_add(h)).min(self.height) as usize;
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let bpp = self.bytes_per_pixel() as usize;
        let stride = (self.stride_pixels as usize).saturating_mul(bpp);
        let copy_bytes = (x1 - x0).saturating_mul(bpp);
        let mut row_y = y0;
        while row_y < y1 {
            let offset = row_y.saturating_mul(stride).saturating_add(x0.saturating_mul(bpp));
            unsafe {
                let src = (self.shadow_addr as usize + offset) as *const u8;
                let dst = (self.virt_addr as usize + offset) as *mut u8;
                let mut b = 0usize;
                while b < copy_bytes {
                    core::ptr::write_volatile(dst.add(b), core::ptr::read(src.add(b)));
                    b += 1;
                }
            }
            row_y += 1;
        }
    }

    pub fn scroll_up_pixels_shadow(
        self,
        x_left: u32,
        x_right: u32,
        y_top: u32,
        y_bottom: u32,
        dy: u32,
        fill: u32,
    ) {
        if dy == 0
            || y_top >= y_bottom
            || y_bottom > self.height
            || dy >= (y_bottom - y_top)
            || x_left >= x_right
            || x_right > self.width
            || !self.is_present()
        {
            return;
        }
        let bpp = self.bytes_per_pixel() as usize;
        let row_stride = (self.stride_pixels as usize).saturating_mul(bpp);
        let row_copy_bytes = ((x_right - x_left) as usize).saturating_mul(bpp);
        let x_offset_bytes = (x_left as usize).saturating_mul(bpp);
        let copy_rows = (y_bottom - y_top).saturating_sub(dy) as usize;
        let base = self.draw_target_base();

        let mut r = 0usize;
        while r < copy_rows {
            let dst_y = (y_top as usize) + r;
            let src_y = dst_y + (dy as usize);
            let dst_addr = base + dst_y * row_stride + x_offset_bytes;
            let src_addr = base + src_y * row_stride + x_offset_bytes;
            unsafe {
                core::ptr::copy(src_addr as *const u8, dst_addr as *mut u8, row_copy_bytes);
            }
            r += 1;
        }
        fill_buffer_rect(
            base,
            row_stride,
            x_left,
            y_bottom - dy,
            x_right,
            y_bottom,
            bpp,
            fill,
        );
    }
}

const ENABLE_VIRTIO_GPU_IPC_NOTIFY: bool = false;

pub fn notify_virtio_gpu_flush(x: u32, y: u32, w: u32, h: u32) {
    if !ENABLE_VIRTIO_GPU_IPC_NOTIFY {
        return;
    }
    #[repr(C)]
    struct VirtioReq {
        sender_pid: u32,
        msg_type: u32,
        payload: [u8; syscall::IPC_INLINE_PAYLOAD_SIZE],
    }
    let mut req = VirtioReq {
        sender_pid: 0,
        msg_type: syscall::VIRTIO_GPU_MSG_TRANSFER_FLUSH,
        payload: [0; syscall::IPC_INLINE_PAYLOAD_SIZE],
    };
    req.payload[0..4].copy_from_slice(&x.to_le_bytes());
    req.payload[4..8].copy_from_slice(&y.to_le_bytes());
    req.payload[8..12].copy_from_slice(&w.to_le_bytes());
    req.payload[12..16].copy_from_slice(&h.to_le_bytes());

    unsafe {
        let _ = syscall::syscall6(
            syscall::SYS_IPC_SEND,
            13,
            &req as *const VirtioReq as u64,
            core::mem::size_of::<VirtioReq>() as u64,
            syscall::IPC_FLAG_TIMEOUT,
            0,
            0,
        );
    }
}

pub fn notify_virtio_gpu_move_cursor(x: u32, y: u32) {
    if !ENABLE_VIRTIO_GPU_IPC_NOTIFY {
        return;
    }
    #[repr(C)]
    struct VirtioCursorReq {
        sender_pid: u32,
        msg_type: u32,
        payload: [u8; syscall::IPC_INLINE_PAYLOAD_SIZE],
    }
    let mut req = VirtioCursorReq {
        sender_pid: 0,
        msg_type: syscall::VIRTIO_GPU_MSG_MOVE_CURSOR,
        payload: [0; syscall::IPC_INLINE_PAYLOAD_SIZE],
    };
    req.payload[0..4].copy_from_slice(&x.to_le_bytes());
    req.payload[4..8].copy_from_slice(&y.to_le_bytes());

    unsafe {
        let _ = syscall::syscall6(
            syscall::SYS_IPC_SEND,
            13,
            &req as *const VirtioCursorReq as u64,
            core::mem::size_of::<VirtioCursorReq>() as u64,
            syscall::IPC_FLAG_TIMEOUT,
            0,
            0,
        );
    }
}
