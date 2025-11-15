// src/c_compat/mod.rs
// Module d'interopérabilité C (FFI)

// Fonctions C du port série (compilées depuis serial.c)
extern "C" {
    /// Initialise le port série COM1 (38400 bauds, 8N1)
    pub fn serial_init();
    
    /// Écrit un caractère sur le port série
    pub fn serial_write_char(c: u8);
    
    /// Écrit une chaîne C sur le port série
    pub fn serial_write_string(s: *const u8);
    
    /// Lit un caractère du port série (bloquant)
    pub fn serial_read_char() -> u8;
    
    /// Vérifie si des données sont disponibles
    pub fn serial_available() -> bool;
}

/// API Rust sûre pour écrire une chaîne sur le port série
pub fn serial_print(s: &str) {
    for &byte in s.as_bytes() {
        unsafe {
            serial_write_char(byte);
        }
    }
}

/// Macro pour println! sur le port série
#[macro_export]
macro_rules! serial_println {
    () => ($crate::c_compat::serial_print("\n"));
    ($($arg:tt)*) => ({
        use alloc::format;
        $crate::c_compat::serial_print(&format!($($arg)*));
        $crate::c_compat::serial_print("\n");
    });
}

#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => ({
        use alloc::format;
        $crate::c_compat::serial_print(&format!($($arg)*));
    });
}