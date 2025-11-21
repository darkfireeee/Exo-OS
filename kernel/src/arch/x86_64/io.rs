use core::arch::asm;
use core::marker::PhantomData;

/// A port I/O wrapper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Port<T> {
    port: u16,
    phantom: PhantomData<T>,
}

impl<T> Port<T> {
    /// Creates a new I/O port with the given port number.
    pub const fn new(port: u16) -> Port<T> {
        Port {
            port,
            phantom: PhantomData,
        }
    }
}

impl Port<u8> {
    /// Reads a byte from the port.
    ///
    /// # Safety
    ///
    /// This function is unsafe because the I/O port could have side effects that violate memory
    /// safety.
    pub unsafe fn read(&mut self) -> u8 {
        let value: u8;
        asm!("in al, dx", out("al") value, in("dx") self.port, options(nomem, nostack, preserves_flags));
        value
    }

    /// Writes a byte to the port.
    ///
    /// # Safety
    ///
    /// This function is unsafe because the I/O port could have side effects that violate memory
    /// safety.
    pub unsafe fn write(&mut self, value: u8) {
        asm!("out dx, al", in("dx") self.port, in("al") value, options(nomem, nostack, preserves_flags));
    }
}

impl Port<u16> {
    /// Reads a word from the port.
    ///
    /// # Safety
    ///
    /// This function is unsafe because the I/O port could have side effects that violate memory
    /// safety.
    pub unsafe fn read(&mut self) -> u16 {
        let value: u16;
        asm!("in ax, dx", out("ax") value, in("dx") self.port, options(nomem, nostack, preserves_flags));
        value
    }

    /// Writes a word to the port.
    ///
    /// # Safety
    ///
    /// This function is unsafe because the I/O port could have side effects that violate memory
    /// safety.
    pub unsafe fn write(&mut self, value: u16) {
        asm!("out dx, ax", in("dx") self.port, in("ax") value, options(nomem, nostack, preserves_flags));
    }
}

impl Port<u32> {
    /// Reads a double word from the port.
    ///
    /// # Safety
    ///
    /// This function is unsafe because the I/O port could have side effects that violate memory
    /// safety.
    pub unsafe fn read(&mut self) -> u32 {
        let value: u32;
        asm!("in eax, dx", out("eax") value, in("dx") self.port, options(nomem, nostack, preserves_flags));
        value
    }

    /// Writes a double word to the port.
    ///
    /// # Safety
    ///
    /// This function is unsafe because the I/O port could have side effects that violate memory
    /// safety.
    pub unsafe fn write(&mut self, value: u32) {
        asm!("out dx, eax", in("dx") self.port, in("eax") value, options(nomem, nostack, preserves_flags));
    }
}

// Helper functions for direct port I/O

/// Read a byte from a port
pub unsafe fn inb(port: u16) -> u8 {
    let mut p = Port::<u8>::new(port);
    p.read()
}

/// Write a byte to a port
pub unsafe fn outb(port: u16, value: u8) {
    let mut p = Port::<u8>::new(port);
    p.write(value)
}

/// Read a word from a port
pub unsafe fn inw(port: u16) -> u16 {
    let mut p = Port::<u16>::new(port);
    p.read()
}

/// Write a word to a port
pub unsafe fn outw(port: u16, value: u16) {
    let mut p = Port::<u16>::new(port);
    p.write(value)
}

/// Read a double word from a port
pub unsafe fn inl(port: u16) -> u32 {
    let mut p = Port::<u32>::new(port);
    p.read()
}

/// Write a double word to a port
pub unsafe fn outl(port: u16, value: u32) {
    let mut p = Port::<u32>::new(port);
    p.write(value)
}
