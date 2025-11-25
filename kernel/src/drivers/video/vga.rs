//! VGA text mode driver.

use crate::drivers::{DeviceInfo, Driver, DriverError, DriverResult};
use core::fmt;
use lazy_static::lazy_static;   
use spin::Mutex;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Color {
    Black = 0,
    Blue = 1,
    Green = 2,
    Cyan = 3,
    Red = 4,
    Magenta = 5,
    Brown = 6,
    LightGray = 7,
    DarkGray = 8,
    LightBlue = 9,
    LightGreen = 10,
    LightCyan = 11,
    LightRed = 12,
    Pink = 13,
    Yellow = 14,
    White = 15,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
struct ColorCode(u8);

impl ColorCode {
    fn new(foreground: Color, background: Color) -> ColorCode {
        ColorCode((background as u8) << 4 | (foreground as u8))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
struct ScreenChar {
    ascii_character: u8,
    color_code: ColorCode,
}

const BUFFER_HEIGHT: usize = 25;
const BUFFER_WIDTH: usize = 80;

#[repr(transparent)]
struct Buffer {
    chars: [[ScreenChar; BUFFER_WIDTH]; BUFFER_HEIGHT],
}

/// VGA driver structure.
pub struct VgaDriver {
    column_position: usize,
    color_code: ColorCode,
    buffer: &'static mut Buffer,
}

impl VgaDriver {
    pub fn new() -> Self {
        Self {
            column_position: 0,
            color_code: ColorCode::new(Color::LightGreen, Color::Black),
            buffer: unsafe { &mut *(0xb8000 as *mut Buffer) },
        }
    }

    pub fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.new_line(),
            byte => {
                if self.column_position >= BUFFER_WIDTH {
                    self.new_line();
                }

                let row = BUFFER_HEIGHT - 1;
                let col = self.column_position;

                let color_code = self.color_code;
                unsafe {
                    core::ptr::write_volatile(
                        &mut self.buffer.chars[row][col] as *mut ScreenChar,
                        ScreenChar {
                            ascii_character: byte,
                            color_code,
                        },
                    );
                }
                self.column_position += 1;
            }
        }
    }

    pub fn write_string(&mut self, s: &str) {
        for byte in s.bytes() {
            match byte {
                // printable ASCII byte or newline
                0x20..=0x7e | b'\n' => self.write_byte(byte),
                // not part of printable ASCII range
                _ => self.write_byte(0xfe),
            }
        }
    }

    fn new_line(&mut self) {
        for row in 1..BUFFER_HEIGHT {
            for col in 0..BUFFER_WIDTH {
                unsafe {
                    let character = core::ptr::read_volatile(&self.buffer.chars[row][col]);
                    core::ptr::write_volatile(&mut self.buffer.chars[row - 1][col], character);
                }
            }
        }
        self.clear_row(BUFFER_HEIGHT - 1);
        self.column_position = 0;
    }

    fn clear_row(&mut self, row: usize) {
        let blank = ScreenChar {
            ascii_character: b' ',
            color_code: self.color_code,
        };
        for col in 0..BUFFER_WIDTH {
            unsafe {
                core::ptr::write_volatile(&mut self.buffer.chars[row][col], blank);
            }
        }
    }

    pub fn clear_screen(&mut self) {
        for row in 0..BUFFER_HEIGHT {
            self.clear_row(row);
        }
        self.column_position = 0;
    }
}

impl Driver for VgaDriver {
    fn name(&self) -> &str {
        "VGA Text Mode Driver"
    }

    fn init(&mut self) -> DriverResult<()> {
        self.clear_screen();
        Ok(())
    }

    fn probe(&self) -> DriverResult<DeviceInfo> {
        Ok(DeviceInfo {
            name: "VGA Controller",
            vendor_id: 0, // Generic
            device_id: 0,
        })
    }
}

impl fmt::Write for VgaDriver {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_string(s);
        Ok(())
    }
}

lazy_static! {
    pub static ref WRITER: Mutex<VgaDriver> = Mutex::new(VgaDriver::new());
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;

    crate::arch::x86_64::without_interrupts(|| {
        WRITER
            .lock()
            .write_fmt(args)
            .expect("Printing to VGA failed");
    });
}

/// Prints to the VGA text buffer.
#[macro_export]
macro_rules! vga_print {
    ($($arg:tt)*) => {
        $crate::drivers::video::vga::_print(format_args!($($arg)*));
    };
}

/// Prints to the VGA text buffer, appending a newline.
#[macro_export]
macro_rules! vga_println {
    () => ($crate::vga_print!("\n"));
    ($fmt:expr) => ($crate::vga_print!(concat!($fmt, "\n")));
    ($fmt:expr, $($arg:tt)*) => ($crate::vga_print!(concat!($fmt, "\n"), $($arg)*));
}
