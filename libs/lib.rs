// libs/lib.rs
// Point d'entrée pour les bibliothèques partagées entre kernel et userland
#![no_std]

// Réexportations des bibliothèques principales
pub use exo_types::{PhysAddr, VirtAddr, Capability, ExoError, Result};
pub use exo_ipc::{Channel, Message};
pub use exo_crypto::{Kyber, Dilithium, ChaCha20};

// Versioning
pub const EXO_LIBS_VERSION: &str = "0.1.0-alpha";

// Initialisation des bibliothèques
pub fn init() {
    exo_types::init();
    exo_crypto::init();
    
    // Logging initialisé après les autres modules
    log::info!("Exo-OS libraries initialized (v{})", EXO_LIBS_VERSION);
}