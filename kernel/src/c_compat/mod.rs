// src/c_compat/mod.rs
// Module d'interopérabilité C (FFI)

use crate::println;

// Indique au linker Rust de lier la bibliothèque statique `libc_compat.a`
// qui est générée par le script `build.rs` à partir des fichiers .c.
#[link(name = "c_compat", kind = "static")]
extern "C" {
    // Déclaration des fonctions C que nous souhaitons appeler depuis Rust.
    // Les noms doivent correspondre exactement aux noms des fonctions dans les fichiers .c.
    
    // Fonctions du pilote série
    pub fn serial_init();
    pub fn serial_write_char(c: u8);

    // Fonctions du pilote PCI
    pub fn pci_init();
    pub fn pci_enumerate_buses();
    
    // Nouvelles fonctions pour l'intégration avec le kernel Rust
    pub fn c_kernel_main(multiboot_info_ptr: u64, multiboot_magic: u32) -> !;
    pub fn c_panic(msg: *const i8);
}

/// Fournit une API Rust sûre et agréable pour écrire une chaîne sur le port série.
/// Cette fonction est sûre car elle garantit que seuls des octets valides sont passés
/// à la fonction C sous-jacente.
pub fn serial_write_str(s: &str) {
    // On itère sur les octets de la chaîne de caractères.
    // L'appel à la fonction C est `unsafe`, mais il est isolé dans cette boucle.
    for &byte in s.as_bytes() {
        unsafe {
            serial_write_char(byte);
        }
    }
}

/// Initialise les pilotes C.
/// Cette fonction sert de point d'entrée unique pour l'initialisation de ce module.
pub fn init() {
    println!("[C_COMPAT] Initialisation de la couche de compatibilité C...");

    unsafe {
        serial_init();
        pci_init();
    }

    println!("[C_COMPAT] Couche C initialisée.");
}

/// Lance l'énumération des bus PCI via le pilote C.
pub fn enumerate_pci() {
    println!("[C_COMPAT] Énumération des périphériques PCI...");
    unsafe {
        pci_enumerate_buses();
    }
    println!("[C_COMPAT] Énumération PCI terminée.");
}

/// Point d'entrée C pour le kernel (appelé depuis boot.c)
pub fn kernel_main_c(multiboot_info_ptr: u64, multiboot_magic: u32) -> ! {
    unsafe {
        c_kernel_main(multiboot_info_ptr, multiboot_magic)
    }
}

/// Fonction de panic pour le code C
pub fn panic_c(msg: &str) {
    use alloc::ffi::CString;
    
    if let Ok(c_msg) = CString::new(msg) {
        unsafe {
            c_panic(c_msg.as_ptr());
        }
    } else {
        unsafe {
            c_panic(b"Invalid UTF-8 panic message\0".as_ptr() as *const i8);
        }
    }
}