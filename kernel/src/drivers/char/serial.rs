//! Serial driver (UART 16550).

use crate::arch::x86_64::serial::SerialPort;
use crate::drivers::{DeviceInfo, Driver, DriverError, DriverResult};
use core::fmt;
use lazy_static::lazy_static;
use spin::Mutex;

/// Serial driver structure.
pub struct SerialDriver {
    port: SerialPort,
}

impl SerialDriver {
    /// Creates a new serial driver for COM1.
    pub const fn new() -> Self {
        Self {
            port: SerialPort::new(0x3F8),
        }
    }

    /// Writes a byte to the serial port.
    pub fn write_byte(&mut self, byte: u8) {
        self.port.send(byte);
    }

    /// Writes a string to the serial port.
    pub fn write_string(&mut self, s: &str) {
        for byte in s.bytes() {
            self.write_byte(byte);
        }
    }
}

impl Driver for SerialDriver {
    fn name(&self) -> &str {
        "UART 16550 Serial Driver"
    }

    fn init(&mut self) -> DriverResult<()> {
        self.port.init();
        Ok(())
    }

    fn probe(&self) -> DriverResult<DeviceInfo> {
        Ok(DeviceInfo {
            name: "COM1",
            vendor_id: 0, // Generic
            device_id: 0x1655, // UART 16550
        })
    }
}

impl fmt::Write for SerialDriver {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_string(s);
        Ok(())
    }
}

lazy_static! {
    pub static ref SERIAL1: Mutex<SerialDriver> = {
        let mut serial = SerialDriver::new();
        let _ = serial.init();
        Mutex::new(serial)
    };
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;

    crate::arch::x86_64::without_interrupts(|| {
        SERIAL1
            .lock()
            .write_fmt(args)
            .expect("Printing to serial failed");
    });
}

/// Prints to the host through the serial interface.
#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::drivers::char::serial::_print(format_args!($($arg)*));
    };
}

/// Prints to the host through the serial interface, appending a newline.
#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($fmt:expr) => ($crate::serial_print!(concat!($fmt, "\n")));
    ($fmt:expr, $($arg:tt)*) => ($crate::serial_print!(concat!($fmt, "\n"), $($arg)*));
}
