//! Abstractions pour l'architecture x86_64
//! 
//! Ce module fournit des abstractions pour les fonctionnalités spécifiques
//! à l'architecture x86_64.

#[cfg(not(target_os = "windows"))]
pub mod registers;
#[cfg(target_os = "windows")]
#[path = "registers_stubs.rs"]
pub mod registers;

pub mod interrupts;

// Réexportations
pub use registers::*;
pub use interrupts::*;