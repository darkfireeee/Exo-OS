//! I/O traits et implémentations robustes
//!
//! Ce module fournit des traits et structures pour les opérations d'entrée/sortie.

pub mod traits;
pub mod stdio;
pub mod cursor;
pub mod buffered;

// Réexportations principales
pub use traits::{Read, Write, Seek, SeekFrom};
pub use stdio::{Stdin, Stdout, Stderr, stdin, stdout, stderr};
pub use cursor::Cursor;
pub use buffered::{BufReader, BufWriter};

/// Type Result spécifique I/O
pub type Result<T> = core::result::Result<T, crate::error::IoError>;
