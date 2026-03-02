// kernel/src/fs/exofs/objects/object_meta.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ObjectMeta — métadonnées étendues d'un objet (droits, timestamps, etc.)
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════

use crate::fs::exofs::objects::logical_object::LogicalObjectDisk;

/// Métadonnées étendues d'un LogicalObject (compatibilité POSIX).
#[derive(Copy, Clone, Debug)]
pub struct ObjectMeta {
    /// Mode bits POSIX (rwx×3 + setuid/setgid/sticky).
    pub mode:  u32,
    /// UID POSIX (0 dans ExoFS, géré par capabilities).
    pub uid:   u32,
    /// GID POSIX (0 dans ExoFS, géré par capabilities).
    pub gid:   u32,
    /// Nombre de liens hard (1 pour les objets non-répertoire).
    pub nlink: u32,
}

impl ObjectMeta {
    /// Crée des métadonnées depuis la structure on-disk.
    pub fn from_disk(disk: &LogicalObjectDisk) -> Self {
        Self {
            mode:  { disk.mode },
            uid:   { disk.uid },
            gid:   { disk.gid },
            nlink: 1,
        }
    }

    /// Métadonnées par défaut pour un nouvel objet.
    pub fn default_for_object(mode: u32) -> Self {
        Self { mode, uid: 0, gid: 0, nlink: 1 }
    }

    /// Mode POSIX régulier (fichier).
    pub const MODE_FILE: u32 = 0o100644;

    /// Mode POSIX répertoire.
    pub const MODE_DIR:  u32 = 0o040755;
}
