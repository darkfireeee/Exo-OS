//! C compatibility layer
//! 
//! Provides Rust interfaces for C drivers and utilities

// External C modules (compiled separately)
// These are implemented in .c files in this directory
pub mod serial {
    extern "C" {
        pub fn serial_init();
        pub fn serial_putc(c: u8);
        pub fn serial_puts(s: *const u8);
        pub fn serial_getc() -> u8;
    }
}

pub mod vga {
    extern "C" {
        pub fn vga_init();
        pub fn vga_clear();
        pub fn vga_putc(c: u8);
        pub fn vga_puts(s: *const u8);
        pub fn vga_set_color(fg: u8, bg: u8);
    }
}

pub mod keyboard {
    extern "C" {
        pub fn keyboard_init();
        pub fn keyboard_getc() -> u8;
        pub fn keyboard_has_input() -> i32;
    }
}

pub mod pci {
    extern "C" {
        pub fn pci_init();
        pub fn pci_read_config_byte(bus: u8, slot: u8, func: u8, offset: u8) -> u8;
        pub fn pci_read_config_word(bus: u8, slot: u8, func: u8, offset: u8) -> u16;
        pub fn pci_read_config_dword(bus: u8, slot: u8, func: u8, offset: u8) -> u32;
        pub fn pci_write_config_byte(bus: u8, slot: u8, func: u8, offset: u8, value: u8);
        pub fn pci_write_config_word(bus: u8, slot: u8, func: u8, offset: u8, value: u16);
        pub fn pci_write_config_dword(bus: u8, slot: u8, func: u8, offset: u8, value: u32);
    }
}

pub mod acpi {
    extern "C" {
        pub fn acpi_init() -> i32;
        pub fn acpi_shutdown();
        pub fn acpi_reboot();
        pub fn acpi_enable();
        pub fn acpi_disable();
    }
}

// Safe Rust wrappers
pub fn init_all() {
    unsafe {
        serial::serial_init();
        vga::vga_init();
        keyboard::keyboard_init();
        pci::pci_init();
        let _ = acpi::acpi_init();
    }
}
