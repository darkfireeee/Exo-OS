//! Macros utilitaires pour le noyau
//! 
//! Ce module fournit des macros utiles pour le développement du noyau,
//! notamment pour le débogage et l'initialisation paresseuse.

pub mod println;
pub mod lazy_static;

// Les macros sont automatiquement exportées via #[macro_export]
// Pas besoin de pub use pour les macros
