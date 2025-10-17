// drivers/serial.rs - Pilote serial UART 16550 simple en Rust pur

use core::fmt;
use crate::arch::x86_64::registers::{write_port_u8, read_port_u8};

const SERIAL_PORT: u16 = 0x3F8; // COM1

pub struct SerialPort {
    port: u16,
}

impl SerialPort {
    pub const fn new(port: u16) -> Self {
        SerialPort { port }
    }

    pub fn init(&self) {
        unsafe {
            write_port_u8(self.port + 1, 0x00);    // Disable interrupts
            write_port_u8(self.port + 3, 0x80);    // Enable DLAB
            write_port_u8(self.port + 0, 0x03);    // Set divisor to 3 (38400 baud)
            write_port_u8(self.port + 1, 0x00);
            write_port_u8(self.port + 3, 0x03);    // 8 bits, no parity, one stop bit
            write_port_u8(self.port + 2, 0xC7);    // Enable FIFO, clear, 14-byte threshold
            write_port_u8(self.port + 4, 0x0B);    // IRQs enabled, RTS/DSR set
        }
    }

    fn is_transmit_empty(&self) -> bool {
        unsafe { read_port_u8(self.port + 5) & 0x20 != 0 }
    }

    pub fn write_byte(&self, byte: u8) {
        while !self.is_transmit_empty() {}
        unsafe {
            write_port_u8(self.port, byte);
        }
    }

    pub fn write_str(&self, s: &str) {
        for byte in s.bytes() {
            self.write_byte(byte);
        }
    }
}

impl fmt::Write for SerialPort {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        SerialPort::write_str(self, s);
        Ok(())
    }
}

pub static SERIAL: SerialPort = SerialPort::new(SERIAL_PORT);

pub fn init() {
    SERIAL.init();
}

pub fn write_char(c: u8) {
    SERIAL.write_byte(c);
}

pub fn write_str(s: &str) {
    SERIAL.write_str(s);
}
