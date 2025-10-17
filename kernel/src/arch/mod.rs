// src/arch/mod.rs
// Abstraction d'architecture - Point d'entrée pour l'architecture spécifique

#[cfg(target_arch = "x86_64")]
pub use self::x86_64::*;

#[cfg(target_arch = "x86_64")]
pub mod x86_64;