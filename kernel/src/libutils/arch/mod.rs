//! Abstractions spécifiques à l'architecture
//! 
//! Ce module fournit des abstractions pour les fonctionnalités spécifiques
//! à l'architecture matérielle.

pub mod x86_64;

// Réexportations
pub use x86_64::{registers, interrupts};