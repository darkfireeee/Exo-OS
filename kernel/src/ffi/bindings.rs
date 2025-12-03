//! External C function bindings
//! 
//! Declares external C functions used by the kernel

use super::types::*;

// External C functions from c_compat module
extern "C" {
    // Serial I/O
    pub fn serial_init();
    pub fn serial_putc(c: c_char);
    pub fn serial_puts(s: *const c_char);
    pub fn serial_getc() -> c_char;
    
    // VGA text mode
    pub fn vga_init();
    pub fn vga_clear();
    pub fn vga_putc(c: c_char);
    pub fn vga_puts(s: *const c_char);
    pub fn vga_set_color(fg: c_uchar, bg: c_uchar);
    
    // PCI bus
    pub fn pci_init();
    pub fn pci_read_config_byte(bus: c_uchar, slot: c_uchar, func: c_uchar, offset: c_uchar) -> c_uchar;
    pub fn pci_read_config_word(bus: c_uchar, slot: c_uchar, func: c_uchar, offset: c_uchar) -> c_ushort;
    pub fn pci_read_config_dword(bus: c_uchar, slot: c_uchar, func: c_uchar, offset: c_uchar) -> c_uint;
    pub fn pci_write_config_byte(bus: c_uchar, slot: c_uchar, func: c_uchar, offset: c_uchar, value: c_uchar);
    pub fn pci_write_config_word(bus: c_uchar, slot: c_uchar, func: c_uchar, offset: c_uchar, value: c_ushort);
    pub fn pci_write_config_dword(bus: c_uchar, slot: c_uchar, func: c_uchar, offset: c_uchar, value: c_uint);
    
    // ACPI
    pub fn acpi_init() -> c_int;
    pub fn acpi_shutdown();
    pub fn acpi_reboot();
    pub fn acpi_enable();
    pub fn acpi_disable();
}

/// Safe wrappers for C functions

/// Initialize serial port
pub fn init_serial() {
    unsafe { serial_init(); }
}

/// Write character to serial
pub fn serial_write_char(c: char) {
    unsafe { serial_putc(c as c_char); }
}

/// Write string to serial
pub fn serial_write_str(s: &str) {
    for c in s.bytes() {
        serial_write_char(c as char);
    }
}

/// Initialize VGA
pub fn init_vga() {
    unsafe { vga_init(); }
}

/// Clear VGA screen
pub fn vga_clear_screen() {
    unsafe { vga_clear(); }
}

/// Write character to VGA
pub fn vga_write_char(c: char) {
    unsafe { vga_putc(c as c_char); }
}

/// Write string to VGA
pub fn vga_write_str(s: &str) {
    for c in s.bytes() {
        vga_write_char(c as char);
    }
}

/// Set VGA color
pub fn vga_set_colors(fg: u8, bg: u8) {
    unsafe { vga_set_color(fg, bg); }
}

/// Initialize PCI bus
pub fn init_pci() {
    unsafe { pci_init(); }
}

/// Initialize ACPI
pub fn init_acpi() -> Result<(), &'static str> {
    let result = unsafe { acpi_init() };
    if result == 0 {
        Ok(())
    } else {
        Err("ACPI initialization failed")
    }
}

/// Shutdown system via ACPI
pub fn shutdown() {
    unsafe { acpi_shutdown(); }
}

/// Reboot system via ACPI
pub fn reboot() {
    unsafe { acpi_reboot(); }
}
