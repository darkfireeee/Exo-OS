/// Structure pour gérer un port I/O série
/// Global COM1 port
static COM1: spin::Mutex<SerialPort> = spin::Mutex::new(SerialPort::new(0x3F8));

/// Initialize serial port system
pub fn init() {
    COM1.lock().init();
}

/// Write byte to COM1
pub fn write_byte(byte: u8) {
    COM1.lock().send(byte);
}

/// Write string to COM1
pub fn write_str(s: &str) {
    let mut port = COM1.lock();
    for byte in s.bytes() {
        port.send(byte);
    }
}

/// Structure pour gérer un port I/O série
pub struct SerialPort {
    base: u16,
}

impl SerialPort {
    pub const fn new(base: u16) -> Self {
        Self { base }
    }

    /// Lit depuis le port data
    #[inline]
    unsafe fn read_data(&self) -> u8 {
        super::inb(self.base)
    }

    /// Écrit vers le port data
    #[inline]
    unsafe fn write_data(&self, value: u8) {
        super::outb(self.base, value);
    }

    /// Lit depuis le port interrupt enable
    #[inline]
    unsafe fn read_int_en(&self) -> u8 {
        super::inb(self.base + 1)
    }

    /// Écrit vers le port interrupt enable
    #[inline]
    unsafe fn write_int_en(&self, value: u8) {
        super::outb(self.base + 1, value);
    }

    /// Écrit vers le port FIFO control
    #[inline]
    unsafe fn write_fifo_ctrl(&self, value: u8) {
        super::outb(self.base + 2, value);
    }

    /// Lit depuis le port line control
    #[inline]
    unsafe fn read_line_ctrl(&self) -> u8 {
        super::inb(self.base + 3)
    }

    /// Écrit vers le port line control
    #[inline]
    unsafe fn write_line_ctrl(&self, value: u8) {
        super::outb(self.base + 3, value);
    }

    /// Écrit vers le port modem control
    #[inline]
    unsafe fn write_modem_ctrl(&self, value: u8) {
        super::outb(self.base + 4, value);
    }

    /// Lit depuis le port line status
    #[inline]
    unsafe fn read_line_sts(&self) -> u8 {
        super::inb(self.base + 5)
    }

    pub fn init(&mut self) {
        unsafe {
            self.write_int_en(0x00);      // Disable interrupts
            self.write_line_ctrl(0x80);   // Enable DLAB (set baud rate divisor)
            self.write_data(0x03);        // Set divisor to 3 (lo byte) 38400 baud
            self.write_int_en(0x00);      //                  (hi byte)
            self.write_line_ctrl(0x03);   // 8 bits, no parity, one stop bit
            self.write_fifo_ctrl(0xC7);   // Enable FIFO, clear them, with 14-byte threshold
            self.write_modem_ctrl(0x0B);  // IRQs enabled, RTS/DSR set
        }
    }

    pub fn send(&mut self, data: u8) {
        unsafe {
            while (self.read_line_sts() & 0x20) == 0 {}
            self.write_data(data);
        }
    }
}
