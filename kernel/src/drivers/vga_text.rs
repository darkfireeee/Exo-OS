//! Simple VGA text-mode fallback writer (no heap, no locks)
//!
//! This file provides a few tiny helpers to write ASCII text directly to
//! the VGA text buffer at physical 0xb8000. It's intentionally minimal and
//! safe to call very early during boot (before the allocator is up).

use core::sync::atomic::{AtomicU8, Ordering};

/// VGA text buffer dimensions
const WIDTH: usize = 80;
const HEIGHT: usize = 25;
const BUFFER_ADDR: usize = 0xb8000;

/// Foreground colors (basic palette)
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Color {
    Black = 0x0,
    Blue = 0x1,
    Green = 0x2,
    Cyan = 0x3,
    Red = 0x4,
    Magenta = 0x5,
    Brown = 0x6,
    LightGray = 0x7,
    DarkGray = 0x8,
    LightBlue = 0x9,
    LightGreen = 0xa,
    LightCyan = 0xb,
    LightRed = 0xc,
    LightMagenta = 0xd,
    Yellow = 0xe,
    White = 0xf,
}

static FG_COLOR: AtomicU8 = AtomicU8::new(Color::LightGray as u8);

#[inline(always)]
fn attr_byte(fg: u8) -> u8 {
    // background = 0 (black), foreground = fg
    fg & 0x0f
}

/// Clear the whole screen using the current foreground color (background black)
pub fn clear_screen() {
    let fg = FG_COLOR.load(Ordering::SeqCst);
    let attr = attr_byte(fg);
    let buf = BUFFER_ADDR as *mut u8;

    // Each cell is two bytes: ASCII, attribute
    for row in 0..HEIGHT {
        for col in 0..WIDTH {
            let idx = (row * WIDTH + col) * 2;
            unsafe {
                // ascii
                core::ptr::write_volatile(buf.add(idx), b' ');
                // attribute
                core::ptr::write_volatile(buf.add(idx + 1), attr);
            }
        }
    }
}

/// Set the foreground color used for subsequent writes
pub fn set_color(c: Color) {
    FG_COLOR.store(c as u8, Ordering::SeqCst);
}

/// Write an ASCII string at the provided row/column. Non-ASCII bytes are
/// replaced with '?'. Writes stop at the end of the line.
pub fn write_str_at(row: usize, col: usize, s: &str) {
    if row >= HEIGHT || col >= WIDTH {
        return;
    }
    let fg = FG_COLOR.load(Ordering::SeqCst);
    let attr = attr_byte(fg);
    let buf = BUFFER_ADDR as *mut u8;

    let mut c = col;
    for &b in s.as_bytes() {
        if c >= WIDTH {
            break;
        }
        let ch = if b.is_ascii() { b } else { b'?' };
        let idx = (row * WIDTH + c) * 2;
        unsafe {
            core::ptr::write_volatile(buf.add(idx), ch);
            core::ptr::write_volatile(buf.add(idx + 1), attr);
        }
        c += 1;
    }
}

/// Write the string centered on the given row (horizontal center)
pub fn write_centered(row: usize, s: &str) {
    let len = s.len();
    let start = if len >= WIDTH { 0 } else { (WIDTH - len) / 2 };
    write_str_at(row, start, s);
}

/// Convenience: write a small banner in the middle of the screen
pub fn write_banner() {
    clear_screen();
    set_color(Color::LightGreen);
    write_centered(10, "EXO-OS KERNEL v0.1.0");
    set_color(Color::White);
}
