#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use core::cell::UnsafeCell;
#[cfg(target_os = "none")]
use core::panic::PanicInfo;
#[cfg(target_os = "none")]
use exo_syscall_abi as syscall;

#[cfg(target_os = "none")]
#[path = "../../../exo-boot/src/display/font.rs"]
mod shared_font;

#[cfg(target_os = "none")]
use shared_font::{glyph_for, FONT_GLYPH_HEIGHT, FONT_GLYPH_WIDTH};

#[cfg(target_os = "none")]
const CHAR_W: u32 = FONT_GLYPH_WIDTH as u32;
#[cfg(target_os = "none")]
const CHAR_H: u32 = FONT_GLYPH_HEIGHT as u32;
#[cfg(target_os = "none")]
const RECV_TIMEOUT_MS: u64 = 25;
#[cfg(target_os = "none")]
const PROGRESSIVE_CLEAR_ROWS: usize = 8;
#[cfg(target_os = "none")]
const MAX_TEXT_ROWS: usize = 128;
#[cfg(target_os = "none")]
const ANSI_GROUND: u8 = 0;
#[cfg(target_os = "none")]
const ANSI_ESC: u8 = 1;
#[cfg(target_os = "none")]
const ANSI_CSI: u8 = 2;

#[cfg(target_os = "none")]
#[derive(Clone, Copy)]
struct Framebuffer {
    virt_addr: u64,
    width: u32,
    height: u32,
    stride_pixels: u32,
    bpp: u32,
    format: u32,
    size_bytes: u64,
}

#[cfg(target_os = "none")]
impl Framebuffer {
    const fn absent() -> Self {
        Self {
            virt_addr: 0,
            width: 0,
            height: 0,
            stride_pixels: 0,
            bpp: 0,
            format: 0,
            size_bytes: 0,
        }
    }

    fn is_present(self) -> bool {
        self.virt_addr != 0
            && self.width != 0
            && self.height != 0
            && self.stride_pixels >= self.width
            && self.size_bytes != 0
            && self.bytes_per_pixel() >= 3
    }

    fn bytes_per_pixel(self) -> u32 {
        ((self.bpp.saturating_add(7)) / 8).max(1)
    }

    fn encode(self, r: u8, g: u8, b: u8) -> u32 {
        match self.format {
            1 => ((b as u32) << 16) | ((g as u32) << 8) | (r as u32),
            _ => ((r as u32) << 16) | ((g as u32) << 8) | (b as u32),
        }
    }

    fn pixel_offset(self, x: u32, y: u32) -> Option<usize> {
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

    fn put_pixel(self, x: u32, y: u32, pixel: u32) {
        let Some(offset) = self.pixel_offset(x, y) else {
            return;
        };
        let addr = (self.virt_addr as usize).saturating_add(offset);
        unsafe {
            match self.bytes_per_pixel() {
                4 => core::ptr::write_volatile(addr as *mut u32, pixel),
                3 => {
                    let bytes = pixel.to_le_bytes();
                    core::ptr::write_volatile(addr as *mut u8, bytes[0]);
                    core::ptr::write_volatile((addr + 1) as *mut u8, bytes[1]);
                    core::ptr::write_volatile((addr + 2) as *mut u8, bytes[2]);
                }
                _ => {}
            }
        }
    }

    fn read_pixel(self, x: u32, y: u32) -> u32 {
        let Some(offset) = self.pixel_offset(x, y) else {
            return 0;
        };
        let addr = (self.virt_addr as usize).saturating_add(offset);
        unsafe {
            match self.bytes_per_pixel() {
                4 => core::ptr::read_volatile(addr as *const u32),
                3 => {
                    let b0 = core::ptr::read_volatile(addr as *const u8);
                    let b1 = core::ptr::read_volatile((addr + 1) as *const u8);
                    let b2 = core::ptr::read_volatile((addr + 2) as *const u8);
                    u32::from_le_bytes([b0, b1, b2, 0])
                }
                _ => 0,
            }
        }
    }

    fn fill_rect(self, x: u32, y: u32, w: u32, h: u32, pixel: u32) {
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
        let base = self.virt_addr as usize;
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
                        if col + 1 < width && addr & 7 == 0 {
                            core::ptr::write_volatile(addr as *mut u64, pixel64);
                            col += 2;
                        } else {
                            core::ptr::write_volatile(addr as *mut u32, pixel);
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
                        core::ptr::write_volatile(addr as *mut u8, bytes[0]);
                        core::ptr::write_volatile((addr + 1) as *mut u8, bytes[1]);
                        core::ptr::write_volatile((addr + 2) as *mut u8, bytes[2]);
                    }
                    col += 1;
                }
                row = row.saturating_add(1);
            }
            return;
        }
        let mut row = y;
        while row < y_end {
            let mut col = x;
            while col < x_end {
                self.put_pixel(col, row, pixel);
                col = col.saturating_add(1);
            }
            row = row.saturating_add(1);
        }
    }

    fn scroll_up_pixels(self, dy: u32, fill: u32) {
        if dy == 0 || dy >= self.height {
            return;
        }
        if !self.is_present() {
            return;
        }
        let bpp = self.bytes_per_pixel() as usize;
        let row_stride = (self.stride_pixels as usize).saturating_mul(bpp);
        let dy_bytes = (dy as usize).saturating_mul(row_stride);
        let copy_rows = self.height.saturating_sub(dy) as usize;
        let copy_bytes = copy_rows.saturating_mul(row_stride);
        if dy_bytes <= self.size_bytes as usize
            && copy_bytes <= (self.size_bytes as usize).saturating_sub(dy_bytes)
        {
            unsafe {
                core::ptr::copy(
                    (self.virt_addr as usize + dy_bytes) as *const u8,
                    self.virt_addr as *mut u8,
                    copy_bytes,
                );
            }
            self.fill_rect(0, self.height - dy, self.width, dy, fill);
            return;
        }
        let mut row = dy;
        while row < self.height {
            let mut col = 0;
            while col < self.width {
                let pixel = self.read_pixel(col, row);
                self.put_pixel(col, row - dy, pixel);
                col = col.saturating_add(1);
            }
            row = row.saturating_add(1);
        }
        self.fill_rect(0, self.height - dy, self.width, dy, fill);
    }
}

#[cfg(target_os = "none")]
struct Console {
    fb: Framebuffer,
    cols: u32,
    rows: u32,
    col: u32,
    row: u32,
    fg: (u8, u8, u8),
    bg: (u8, u8, u8),
    cursor_visible: bool,
    cursor_drawn: bool,
    clear_generation: u32,
    clear_cursor: u32,
    row_generation: [u32; MAX_TEXT_ROWS],
    ansi_state: u8,
    params: [u16; 4],
    param_count: usize,
}

#[cfg(target_os = "none")]
impl Console {
    const fn new() -> Self {
        Self {
            fb: Framebuffer::absent(),
            cols: 0,
            rows: 0,
            col: 0,
            row: 0,
            fg: (0xe6, 0xf1, 0xff),
            bg: (0x03, 0x0d, 0x14),
            cursor_visible: true,
            cursor_drawn: false,
            clear_generation: 1,
            clear_cursor: 0,
            row_generation: [0; MAX_TEXT_ROWS],
            ansi_state: ANSI_GROUND,
            params: [0; 4],
            param_count: 0,
        }
    }

    fn configure(&mut self, fb: Framebuffer) {
        self.fb = fb;
        self.cols = (fb.width / CHAR_W).max(1);
        self.rows = (fb.height / CHAR_H).max(1);
        self.col = 0;
        self.row = 0;
        self.ansi_state = ANSI_GROUND;
        self.params = [0; 4];
        self.param_count = 0;
        self.begin_clear(false);
    }

    fn begin_clear(&mut self, immediate: bool) {
        self.col = 0;
        self.row = 0;
        self.cursor_drawn = false;
        self.clear_generation = self.clear_generation.wrapping_add(1).max(1);
        self.clear_cursor = 0;
        self.row_generation = [0; MAX_TEXT_ROWS];
        if immediate && self.fb.is_present() {
            let bg = self.fb.encode(self.bg.0, self.bg.1, self.bg.2);
            self.fb.fill_rect(0, 0, self.fb.width, self.fb.height, bg);
            let limit = self.rows.min(MAX_TEXT_ROWS as u32);
            let mut idx = 0u32;
            while idx < limit {
                self.row_generation[idx as usize] = self.clear_generation;
                idx = idx.saturating_add(1);
            }
            self.clear_cursor = limit;
        }
    }

    fn clear(&mut self) {
        self.begin_clear(false);
    }

    fn clear_text_row_index(&mut self, row_idx: u32) {
        if !self.fb.is_present() {
            return;
        }
        let idx = row_idx as usize;
        if idx >= MAX_TEXT_ROWS || self.row_generation[idx] == self.clear_generation {
            return;
        }
        let bg = self.fb.encode(self.bg.0, self.bg.1, self.bg.2);
        self.fb
            .fill_rect(0, row_idx.saturating_mul(CHAR_H), self.fb.width, CHAR_H, bg);
        self.row_generation[idx] = self.clear_generation;
    }

    fn progress_clear(&mut self, max_rows: usize) {
        let limit = self.rows.min(MAX_TEXT_ROWS as u32);
        let mut cleared = 0usize;
        while self.clear_cursor < limit && cleared < max_rows {
            self.clear_text_row_index(self.clear_cursor);
            self.clear_cursor = self.clear_cursor.saturating_add(1);
            cleared += 1;
        }
    }

    fn clear_text_row_once(&mut self) {
        if !self.fb.is_present() {
            return;
        }
        self.clear_text_row_index(self.row);
    }

    fn draw_cursor(&mut self, visible: bool) {
        if !self.fb.is_present() || !self.cursor_visible {
            self.cursor_drawn = false;
            return;
        }
        let x = self
            .col
            .min(self.cols.saturating_sub(1))
            .saturating_mul(CHAR_W);
        let y = self
            .row
            .min(self.rows.saturating_sub(1))
            .saturating_mul(CHAR_H)
            .saturating_add(CHAR_H.saturating_sub(2));
        let pixel = if visible {
            self.fb.encode(self.fg.0, self.fg.1, self.fg.2)
        } else {
            self.fb.encode(self.bg.0, self.bg.1, self.bg.2)
        };
        self.fb.fill_rect(x, y, CHAR_W, 2, pixel);
        self.cursor_drawn = visible;
    }

    fn scroll_lines(&mut self, lines: u32) {
        if !self.fb.is_present() {
            return;
        }
        if self.cursor_drawn {
            self.draw_cursor(false);
        }
        let bg = self.fb.encode(self.bg.0, self.bg.1, self.bg.2);
        let mut remaining = lines.max(1);
        while remaining != 0 {
            self.fb.scroll_up_pixels(CHAR_H, bg);
            remaining -= 1;
        }
    }

    fn newline(&mut self) {
        self.col = 0;
        if self.row + 1 < self.rows {
            self.row += 1;
        } else {
            self.scroll_lines(1);
        }
    }

    fn draw_ascii(&mut self, byte: u8) {
        if !self.fb.is_present() {
            return;
        }
        self.clear_text_row_once();
        let fg = self.fb.encode(self.fg.0, self.fg.1, self.fg.2);
        let bg = self.fb.encode(self.bg.0, self.bg.1, self.bg.2);
        let x = self.col.saturating_mul(CHAR_W);
        let y = self.row.saturating_mul(CHAR_H);
        if byte == b' ' {
            self.fb.fill_rect(x, y, CHAR_W, CHAR_H, bg);
            self.col += 1;
            if self.col >= self.cols {
                self.newline();
            }
            return;
        }

        let glyph = glyph_for(byte);
        let bpp = self.fb.bytes_per_pixel() as usize;
        let stride = (self.fb.stride_pixels as usize).saturating_mul(bpp);
        let base = self.fb.virt_addr as usize;
        let mut gy = 0usize;
        while gy < FONT_GLYPH_HEIGHT {
            let bits = glyph[gy];
            let row_y = y.saturating_add(gy as u32);
            if row_y < self.fb.height {
                let row_base = base
                    .saturating_add((row_y as usize).saturating_mul(stride))
                    .saturating_add((x as usize).saturating_mul(bpp));
                let mut gx = 0usize;
                while gx < FONT_GLYPH_WIDTH {
                    let px = x.saturating_add(gx as u32);
                    if px < self.fb.width {
                        let mask = 0x80 >> gx;
                        if bits & mask != 0 {
                            unsafe {
                                match bpp {
                                    4 => {
                                        core::ptr::write_volatile(
                                            (row_base + gx * 4) as *mut u32,
                                            fg,
                                        );
                                    }
                                    3 => {
                                        let bytes = fg.to_le_bytes();
                                        let addr = row_base + gx * 3;
                                        core::ptr::write_volatile(addr as *mut u8, bytes[0]);
                                        core::ptr::write_volatile((addr + 1) as *mut u8, bytes[1]);
                                        core::ptr::write_volatile((addr + 2) as *mut u8, bytes[2]);
                                    }
                                    _ => self.fb.put_pixel(px, row_y, fg),
                                }
                            }
                        }
                    }
                    gx += 1;
                }
            }
            gy += 1;
        }
        self.col += 1;
        if self.col >= self.cols {
            self.newline();
        }
    }

    fn reset_ansi(&mut self) {
        self.ansi_state = ANSI_GROUND;
        self.params = [0; 4];
        self.param_count = 0;
    }

    fn write_byte(&mut self, byte: u8) {
        match self.ansi_state {
            ANSI_ESC => {
                if byte == b'[' {
                    self.ansi_state = ANSI_CSI;
                    self.params = [0; 4];
                    self.param_count = 0;
                } else {
                    self.reset_ansi();
                }
                return;
            }
            ANSI_CSI => {
                self.write_csi_byte(byte);
                return;
            }
            _ => {}
        }

        match byte {
            0x1b => self.ansi_state = ANSI_ESC,
            0x0c => self.clear(),
            b'\r' => self.col = 0,
            b'\n' => self.newline(),
            0x08 | 0x7f => {
                if self.col > 0 {
                    self.col -= 1;
                }
            }
            b'\t' => {
                let mut n = 4 - (self.col % 4);
                while n != 0 {
                    self.draw_ascii(b' ');
                    n -= 1;
                }
            }
            byte if byte.is_ascii_graphic() || byte == b' ' => self.draw_ascii(byte),
            _ => {}
        }
    }

    fn write_csi_byte(&mut self, byte: u8) {
        if byte.is_ascii_digit() {
            let idx = self.param_count.min(self.params.len() - 1);
            self.params[idx] = self.params[idx]
                .saturating_mul(10)
                .saturating_add((byte - b'0') as u16);
            return;
        }
        if byte == b';' {
            if self.param_count + 1 < self.params.len() {
                self.param_count += 1;
            }
            return;
        }

        match byte {
            b'H' | b'f' => {
                let row = self.params[0].saturating_sub(1) as u32;
                let col = self.params[1].saturating_sub(1) as u32;
                self.row = row.min(self.rows.saturating_sub(1));
                self.col = col.min(self.cols.saturating_sub(1));
            }
            b'J' => {
                if self.params[0] == 2 || self.params[0] == 0 {
                    self.clear();
                }
            }
            b'K' => {
                let bg = self.fb.encode(self.bg.0, self.bg.1, self.bg.2);
                self.fb.fill_rect(
                    self.col.saturating_mul(CHAR_W),
                    self.row.saturating_mul(CHAR_H),
                    self.fb.width,
                    CHAR_H,
                    bg,
                );
            }
            b'm' => self.apply_sgr(),
            _ => {}
        }
        self.reset_ansi();
    }

    fn apply_sgr(&mut self) {
        let count = self.param_count + 1;
        let mut i = 0usize;
        while i < count {
            match self.params[i] {
                0 => {
                    self.fg = (0xe6, 0xf1, 0xff);
                    self.bg = (0x03, 0x0d, 0x14);
                }
                30 => self.fg = (0x00, 0x00, 0x00),
                31 => self.fg = (0xff, 0x6b, 0x6b),
                32 => self.fg = (0x60, 0xff, 0x90),
                33 => self.fg = (0xff, 0xd1, 0x66),
                34 => self.fg = (0x71, 0xa7, 0xff),
                35 => self.fg = (0xd6, 0x7b, 0xff),
                36 => self.fg = (0x62, 0xe6, 0xff),
                37 => self.fg = (0xe6, 0xf1, 0xff),
                _ => {}
            }
            i += 1;
        }
    }

    fn write_all(&mut self, bytes: &[u8]) {
        if !self.fb.is_present() {
            fallback_write(bytes);
            return;
        }
        if self.cursor_drawn {
            self.draw_cursor(false);
        }
        for &byte in bytes {
            self.write_byte(byte);
        }
        self.draw_cursor(true);
    }
}

#[cfg(target_os = "none")]
struct ConsoleCell(UnsafeCell<Console>);

#[cfg(target_os = "none")]
unsafe impl Sync for ConsoleCell {}

#[cfg(target_os = "none")]
static CONSOLE: ConsoleCell = ConsoleCell(UnsafeCell::new(Console::new()));

#[cfg(target_os = "none")]
fn console_mut() -> &'static mut Console {
    unsafe { &mut *CONSOLE.0.get() }
}

#[cfg(target_os = "none")]
fn fallback_write(bytes: &[u8]) {
    let _ = bytes;
}

#[cfg(target_os = "none")]
fn debug_log(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    unsafe {
        let _ = syscall::syscall3(
            syscall::SYS_EXO_LOG,
            bytes.as_ptr() as u64,
            bytes.len() as u64,
            1,
        );
    }
}

#[cfg(target_os = "none")]
fn boot_log(bytes: &[u8]) {
    debug_log(bytes);
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
fn register_endpoint() -> bool {
    let name = b"fb_server";
    let rc = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            syscall::FB_SERVER_ENDPOINT,
        )
    };
    rc >= 0
}

#[cfg(target_os = "none")]
fn map_framebuffer() -> bool {
    let mut info = syscall::FramebufferInfoWire::default();
    boot_log(b"fb_server: framebuffer_info call\n");
    let info_rc = unsafe {
        syscall::syscall1(
            syscall::SYS_FRAMEBUFFER_INFO,
            &mut info as *mut syscall::FramebufferInfoWire as u64,
        )
    };
    boot_log(b"fb_server: framebuffer_info returned\n");
    if info_rc < 0
        || info.phys_addr == 0
        || info.width == 0
        || info.height == 0
        || info.size_bytes == 0
    {
        boot_log(b"fb_server: framebuffer_info invalid\n");
        return false;
    }

    boot_log(b"fb_server: mmio_map call\n");
    let virt = unsafe { syscall::syscall2(syscall::SYS_MMIO_MAP, info.phys_addr, info.size_bytes) };
    boot_log(b"fb_server: mmio_map returned\n");
    if virt <= 0 {
        boot_log(b"fb_server: mmio_map failed\n");
        return false;
    }

    console_mut().configure(Framebuffer {
        virt_addr: virt as u64,
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
fn send_reply(endpoint: u64, status: i64, len: u32) {
    if endpoint == 0 {
        return;
    }
    let reply = syscall::FbReply {
        status,
        len,
        _pad: 0,
    };
    let _ = unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            endpoint,
            &reply as *const syscall::FbReply as u64,
            core::mem::size_of::<syscall::FbReply>() as u64,
            0,
            0,
            0,
        )
    };
}

#[cfg(target_os = "none")]
fn handle(req: &syscall::FbRequest) -> syscall::FbReply {
    match req.msg_type {
        syscall::FB_MSG_WRITE => {
            let n = core::cmp::min(req.a as usize, syscall::FB_TEXT_MAX);
            console_mut().write_all(&req.data[..n]);
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
            console.draw_cursor(true);
            syscall::FbReply::default()
        }
        _ => syscall::FbReply {
            status: syscall::EINVAL,
            len: 0,
            _pad: 0,
        },
    }
}

#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn _start() -> ! {
    boot_log(b"fb_server: boot\n");
    if !register_endpoint() {
        boot_log(b"fb_server: register failed\n");
        exit_failed();
    }
    boot_log(b"fb_server: registered\n");
    boot_log(b"fb_server: mapping framebuffer\n");
    if map_framebuffer() {
        debug_log(b"fb_server: framebuffer mapped\n");
        console_mut().write_all(b"fb_server: framebuffer mapped\n");
    } else {
        boot_log(b"fb_server: framebuffer unavailable, console fallback active\n");
    }

    let mut req = syscall::FbRequest::zeroed();
    loop {
        console_mut().progress_clear(PROGRESSIVE_CLEAR_ROWS);
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
        let reply = handle(&req);
        console_mut().progress_clear(PROGRESSIVE_CLEAR_ROWS);
        send_reply(req.reply_endpoint, reply.status, reply.len);
    }
}

#[cfg(not(target_os = "none"))]
fn main() {}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
