//! Structures de données sans allocation
//! 
//! Ce module fournit des structures de données de base adaptées à un environnement
//! de noyau où l'allocation dynamique peut être limitée ou contrôlée.

pub mod vec;
pub mod string;

// Réexportations
pub use vec::Vec;
pub use string::String;