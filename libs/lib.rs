// libs/lib.rs
// Point d'entrée pour les bibliothèques partagées entre kernel et userland
#![no_std]

// Réexportations des bibliothèques principales
pub use exo_crypto::ChaCha20;
pub use exo_ipc::{Channel, Message};
pub use exo_types::{Capability, ExoError, PhysAddr, Result, VirtAddr};

// Versioning
pub const EXO_LIBS_VERSION: &str = "0.3.0-alpha";