// src/c_compat/mod.rs
// Module d'interopérabilité C (FFI)

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
    println!("Initialisation de la couche de compatibilité C...");

    unsafe {
        serial_init();
        pci_init();
    }

    println!("Couche C initialisée.");
}

/// Lance l'énumération des bus PCI via le pilote C.
pub fn enumerate_pci() {
    println!("Énumération des périphériques PCI...");
    unsafe {
        pci_enumerate_buses();
    }
    println!("Énumération PCI terminée.");
}