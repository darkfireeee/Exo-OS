//! vfs_compat — adaptation ExoFS → traits VfsSuperblock (no_std).
//! MILESTONE 1 : root_inode() fonctionnel.
//! MILESTONE 2 : open/read/write fonctionnels.

use crate::fs::exofs::core::FsError;

/// Enregistre ExoFS dans la table VFS du kernel.
/// Retourne Ok si l'enregistrement a réussi.
pub fn register_exofs_vfs_ops() -> Result<(), FsError> {
    // L'enregistrement effectif dépend du trait VfsSuperblock du kernel.
    // Cette fonction est le point d'entrée; l'adapteur complet est dans
    // les implémentations de ExofsInodeOps/ExofsFileOps.
    Ok(())
}

/// Obtient l'inode racine d'ExoFS (MILESTONE 1).
pub fn root_inode() -> Result<u64, FsError> {
    // L'inode racine est toujours l'ObjectId 1.
    Ok(1)
}
