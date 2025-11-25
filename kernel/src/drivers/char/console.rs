//! Console abstraction.

use super::serial::SERIAL1;
use core::fmt;
use lazy_static::lazy_static;
use spin::Mutex;

/// Console structure.
pub struct Console;

impl Console {
    /// Writes a string to the console.
    pub fn write_string(&self, s: &str) {
        use core::fmt::Write;
        // For now, redirect to serial
        // In the future, this could write to VGA buffer too
        x86_64::instructions::interrupts::without_interrupts(|| {
            let mut serial = SERIAL1.lock();
            let _ = serial.write_str(s);
        });
    }
}

impl fmt::Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_string(s);
        Ok(())
    }
}

lazy_static! {
    /// Global console instance.
    pub static ref CONSOLE: Mutex<Console> = Mutex::new(Console);
}

/// Prints to the console.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::drivers::char::console::_print(format_args!($($arg)*)));
}

/// Prints to the console, appending a newline.
#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    x86_64::instructions::interrupts::without_interrupts(|| {
        CONSOLE
            .lock()
            .write_fmt(args)
            .expect("Printing to console failed");
    });
}
