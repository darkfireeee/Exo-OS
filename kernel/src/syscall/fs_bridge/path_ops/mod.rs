//! # syscall/fs_bridge/path_ops/mod.rs — Résolution de chemins, arborescence, symlinks et métadonnées
//!
//! Ce module ségrègue la logique de traversée et manipulation du système de fichiers en sous-modules :
//! - `resolution` : Normalisation des chemins, hash Blake3, parcours et indexation de répertoire.
//! - `operations` : Primitives VFS POSIX (open, stat, lstat, fstat, mkdir, rmdir, unlink, rename, link).
//! - `symlinks`   : Gestion et résolution des liens symboliques POSIX (symlink, readlink).
//! - `metadata`   : Lecture de répertoires (getdents64), droits, attributs, statx, synchronisation.

pub mod metadata;
pub mod operations;
pub mod resolution;
pub mod symlinks;

pub use metadata::*;
pub use operations::*;
pub use resolution::*;
pub use symlinks::*;
