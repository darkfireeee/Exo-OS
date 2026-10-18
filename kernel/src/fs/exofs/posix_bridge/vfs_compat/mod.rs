//! # kernel::fs::exofs::posix_bridge::vfs_compat — Couche de compatibilité VFS
//!
//! Organisation modulaire ségréguée :
//! - `types`     : Constantes POSIX, structures VfsInode, VfsDirent, VfsFd
//! - `fd_table`  : Table des descripteurs de fichiers ouverts et isolation par PID
//! - `directory` : Registre des répertoires et hiérarchie en mémoire
//! - `persist`   : Persistance et sérialisation du namespace sur disque ExoFS
//! - `ops`       : Surface POSIX (lookup, create, open, read, write, getattr, readdir, ...)
//! - `helpers`   : Helpers internes de conversion d'inodes, hashing et validation de noms
//! - `tests`     : Tests unitaires de conformité

pub mod types;
pub mod fd_table;
pub mod directory;
pub mod persist;
pub mod ops;
pub mod helpers;
#[cfg(test)]
pub mod tests;

pub use types::*;
pub use fd_table::*;
pub use directory::*;
pub use persist::*;
pub use ops::*;
pub use helpers::*;
