//! Primitifs de synchronisation adaptées pour le noyau
//! 
//! Ce module fournit des structures de synchronisation qui fonctionnent dans un environnement
//! no_std et sont optimisées pour les performances du noyau.

pub mod mutex;
pub mod once;

// Réexportations
pub use mutex::Mutex;
pub use once::Once;