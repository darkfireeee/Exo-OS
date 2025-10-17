//! Wrappers sûrs pour les appels C
//! 
//! Ce module fournit des wrappers sûrs pour interagir avec du code C
//! depuis Rust, en particulier pour les chaînes et les listes d'arguments variables.

pub mod c_str;
pub mod va_list;

// Réexportations
pub use c_str::CStr;
pub use va_list::VaList;