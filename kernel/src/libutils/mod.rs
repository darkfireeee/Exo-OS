//! Bibliothèque de modules réutilisables pour le noyau
//! 
//! Ce module contient des abstractions et des utilitaires communs
//! utilisés à travers tout le noyau.

// Déclaration des sous-modules
// pub mod collections;  // Temporairement désactivé - nécessite Vec et String d'alloc
pub mod sync;
// pub mod memory;  // Temporairement désactivé - conflits avec kernel/src/memory
pub mod arch;
pub mod macros;
pub mod display;
pub mod ffi;  // Réactivé pour l'interopérabilité C/Rust

// Réexportations pour un accès facile
// pub use collections::*;
pub use sync::*;
// pub use memory::*;
pub use arch::*;
pub use macros::*;
pub use display::*;
pub use ffi::*;
