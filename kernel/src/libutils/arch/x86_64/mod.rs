//! Abstractions pour l'architecture x86_64
//! 
//! Ce module fournit des abstractions pour les fonctionnalités spécifiques
//! à l'architecture x86_64.

pub mod registers;
pub mod interrupts;

// Réexportations
pub use registers::*;
pub use interrupts::*;