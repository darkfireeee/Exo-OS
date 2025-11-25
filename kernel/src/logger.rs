//! Simple logger implementation for early boot
//! 
//! Provides logging to serial port and VGA before the full system is initialized.

use log::{Level, Metadata, Record, LevelFilter};

/// Simple logger that writes to serial port
struct SimpleLogger;

impl log::Log for SimpleLogger {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            // Format: [LEVEL] message
            let level_str = match record.level() {
                Level::Error => "ERROR",
                Level::Warn  => "WARN ",
                Level::Info  => "INFO ",
                Level::Debug => "DEBUG",
                Level::Trace => "TRACE",
            };
            
            // Write to serial via C function
            unsafe {
                // Write level prefix
                serial_write_str("[");
                serial_write_str(level_str);
                serial_write_str("] ");
                
                // Write message - convert args to string manually
                use core::fmt::Write;
                let mut buf = [0u8; 512];
                let pos = {
                    let mut writer = BufferWriter { buffer: &mut buf, pos: 0 };
                    let _ = core::write!(&mut writer, "{}\n", record.args());
                    writer.pos
                };
                serial_write_buf(&buf[..pos]);
            }
        }
    }

    fn flush(&self) {}
}

/// Simple buffer writer for formatting without alloc
pub struct BufferWriter<'a> {
    pub buffer: &'a mut [u8],
    pub pos: usize,
}

impl<'a> core::fmt::Write for BufferWriter<'a> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let bytes = s.as_bytes();
        let remaining = self.buffer.len() - self.pos;
        let to_write = bytes.len().min(remaining);
        
        if to_write > 0 {
            self.buffer[self.pos..self.pos + to_write].copy_from_slice(&bytes[..to_write]);
            self.pos += to_write;
        }
        
        Ok(())
    }
}

/// Write string to serial port via C stubs
unsafe fn serial_write_str(s: &str) {
    serial_write_buf(s.as_bytes());
}

/// Write buffer to serial port via C stubs
pub unsafe fn serial_write_buf(bytes: &[u8]) {
    extern "C" {
        fn serial_puts(s: *const u8);
    }
    
    // Ensure null termination
    let mut buf = [0u8; 512];
    let len = bytes.len().min(511);
    buf[..len].copy_from_slice(&bytes[..len]);
    buf[len] = 0;
    
    serial_puts(buf.as_ptr());
}

/// Global logger instance
static LOGGER: SimpleLogger = SimpleLogger;

/// Initialize the logger
/// 
/// Call this very early in boot process (from boot_sequence)
pub fn init() {
    early_print("[LOGGER] Setting logger...\n");
    match log::set_logger(&LOGGER) {
        Ok(_) => {
            log::set_max_level(LevelFilter::Info);
            early_print("[LOGGER] Logger initialized successfully!\n");
        }
        Err(e) => {
            early_print("[LOGGER] ERROR: Failed to set logger: ");
            use core::fmt::Write;
            let mut buf = [0u8; 256];
            let mut writer = BufferWriter { buffer: &mut buf, pos: 0 };
            let _ = core::write!(&mut writer, "{}\n", e);
            let pos = writer.pos;
            unsafe { serial_write_buf(&buf[..pos]); }
        }
    }
}

/// Initialize with specific log level
pub fn init_with_level(level: LevelFilter) {
    log::set_logger(&LOGGER)
        .map(|()| log::set_max_level(level))
        .expect("Failed to set logger");
}

/// Print directly to serial (bypass logger for early debug)
pub fn early_print(s: &str) {
    unsafe {
        serial_write_str(s);
    }
}

/// Log at DEBUG level
#[inline]
pub fn debug(msg: &str) {
    log::debug!("{}", msg);
}

/// Log at INFO level
#[inline]
pub fn info(msg: &str) {
    log::info!("{}", msg);
}

/// Log at WARN level
#[inline]
pub fn warn(msg: &str) {
    log::warn!("{}", msg);
}

/// Log at ERROR level
#[inline]
pub fn error(msg: &str) {
    log::error!("{}", msg);
}
