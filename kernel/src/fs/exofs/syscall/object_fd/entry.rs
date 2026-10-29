//! # fs/exofs/syscall/object_fd/entry.rs — Entrée de descripteur d'objet ExoFS
//!
//! Représentation d'un slot ouvert dans la table plate sans allocation.

use crate::fs::exofs::core::types::BlobId;
use super::flags as open_flags;

/// Fd réservés : 0 (stdin), 1 (stdout), 2 (stderr), 3 (exofs-ctrl).
pub const FD_RESERVED: u32 = 4;
/// Nombre de slots réellement alloués dans la table statique.
pub const MAX_FDS: usize = 4096;
/// Fd maximum inclusif.
pub const FD_MAX: u32 = FD_RESERVED + MAX_FDS as u32 - 1;
/// Marqueur de slot libre.
pub const SLOT_FREE: u32 = u32::MAX;

#[inline]
pub fn current_owner_pid() -> u64 {
    #[cfg(target_os = "none")]
    {
        crate::syscall::fast_path::syscall_current_pid() as u64
    }
    #[cfg(not(target_os = "none"))]
    {
        0
    }
}

#[inline]
pub fn owner_matches(entry: ObjectFdEntry, owner_pid: u64) -> bool {
    #[cfg(target_os = "none")]
    {
        owner_pid == 1 || (entry.owner_uid != 0 && entry.owner_uid == owner_pid)
    }
    #[cfg(not(target_os = "none"))]
    {
        owner_pid == 0 || entry.owner_uid == 0 || entry.owner_uid == owner_pid
    }
}

/// Un descripteur de fichier ExoFS ouvert.
#[derive(Clone, Copy)]
pub struct ObjectFdEntry {
    /// Numéro de fd (= index + FD_RESERVED).
    pub fd: u32,
    /// fd canonique qui porte l'état partagé de l'open-file-description.
    pub shared_fd: u32,
    /// BlobId du blob associé.
    pub blob_id: BlobId,
    /// Flags d'ouverture (O_RDONLY, O_WRONLY, O_RDWR, …).
    pub flags: u32,
    /// Curseur de lecture/écriture courant.
    pub cursor: u64,
    /// Taille connue du blob en octets (0 = inconnue).
    pub size: u64,
    /// Compteur de références (fork/dup).
    pub ref_count: u32,
    /// Epoch au moment de l'ouverture.
    pub epoch_id: u64,
    /// Uid de l'appelant (pour vérification de droits ultérieure).
    pub owner_uid: u64,
}

impl ObjectFdEntry {
    pub const fn empty() -> Self {
        Self {
            fd: SLOT_FREE,
            shared_fd: SLOT_FREE,
            blob_id: BlobId([0u8; 32]),
            flags: 0,
            cursor: 0,
            size: 0,
            ref_count: 0,
            epoch_id: 0,
            owner_uid: 0,
        }
    }

    #[inline]
    pub fn is_free(&self) -> bool {
        self.fd == SLOT_FREE
    }

    #[inline]
    pub fn can_read(&self) -> bool {
        open_flags::can_read(self.flags)
    }

    #[inline]
    pub fn can_write(&self) -> bool {
        open_flags::can_write(self.flags)
    }

    #[inline]
    pub fn is_canonical(&self) -> bool {
        self.fd == self.shared_fd
    }

    /// Avance le curseur de `n` octets (saturating).
    #[inline]
    pub fn advance_cursor(&mut self, n: u64) {
        self.cursor = self.cursor.saturating_add(n);
    }
}
