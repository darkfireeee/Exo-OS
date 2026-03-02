// kernel/src/fs/exofs/epoch/epoch_root.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// EpochRoot — liste des objets modifiés dans un Epoch (variable, multi-pages)
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE CHAIN-01 : Chaque page chainée vérifie son propre magic + checksum AVANT lecture.
// RÈGLE EPOCH-07 : magic 0x45504F43 ("EPOC") dans chaque page chainée.
// RÈGLE OOM-02   : try_reserve(1)? avant push().

use core::mem::size_of;
use alloc::vec::Vec;

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, EpochId, DiskOffset, ObjectId,
    EPOCH_ROOT_MAGIC, blake3_hash,
};
use crate::fs::exofs::core::flags::EpochFlags;

// ─────────────────────────────────────────────────────────────────────────────
// EpochRootPageHeader — 80 octets, tête de chaque page chainée
// ─────────────────────────────────────────────────────────────────────────────

/// En-tête de chaque page de l'EpochRoot (on-disk).
///
/// Chaque page contient : header + liste d'EpochRootEntry + checksum final.
#[derive(Copy, Clone)]
#[repr(C, packed)]
pub struct EpochRootPageHeader {
    /// Magic EPOC : 0x45504F43.
    pub magic:          u32,
    /// Version.
    pub version:        u16,
    /// Flags de l'epoch.
    pub flags:          u16,
    /// Identifiant de l'epoch.
    pub epoch_id:       u64,
    /// Nombre d'entrées dans cette page.
    pub entry_count:    u32,
    /// Index de cette page dans la chaîne (0-based).
    pub page_index:     u32,
    /// Offset de la page suivante (0 = fin de chaîne).
    pub next_page:      u64,
    /// Checksum Blake3 de cette page (header + entries).
    pub checksum:       [u8; 32],
}

const _: () = assert!(
    size_of::<EpochRootPageHeader>() == 64,
    "EpochRootPageHeader doit être 64 octets"
);

// ─────────────────────────────────────────────────────────────────────────────
// EpochRootEntry — un objet modifié ou supprimé dans l'epoch
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée d'un objet dans l'EpochRoot.
#[derive(Copy, Clone, Debug)]
#[repr(C, packed)]
pub struct EpochRootEntry {
    /// ObjectId de l'objet modifié.
    pub object_id:    [u8; 32],
    /// Offset disque de sa nouvelle version.
    pub disk_offset:  u64,
    /// Flags (bit 0 = supprimé, bit 1 = créé, bit 2 = modifié).
    pub entry_flags:  u8,
    /// _pad pour alignement.
    pub _pad:         [u8; 7],
}

const _: () = assert!(
    size_of::<EpochRootEntry>() == 48,
    "EpochRootEntry doit être 48 octets"
);

impl EpochRootEntry {
    pub const FLAG_DELETED:  u8 = 1 << 0;
    pub const FLAG_CREATED:  u8 = 1 << 1;
    pub const FLAG_MODIFIED: u8 = 1 << 2;

    pub fn is_deleted(self) -> bool {
        self.entry_flags & Self::FLAG_DELETED != 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// EpochRootInMemory — accumule les modifications avant écriture
// ─────────────────────────────────────────────────────────────────────────────

/// EpochRoot en mémoire vive — accumule les entrées de l'epoch courant.
pub struct EpochRootInMemory {
    /// Identifiant de l'epoch courant.
    pub epoch_id:          EpochId,
    /// Flags de l'epoch.
    pub flags:             EpochFlags,
    /// Objets modifiés ou créés dans cet epoch.
    pub modified_objects:  Vec<EpochRootEntry>,
    /// Objets supprimés dans cet epoch.
    pub deleted_objects:   Vec<ObjectId>,
}

impl EpochRootInMemory {
    /// Crée un EpochRoot vide pour l'epoch donné.
    pub fn new(epoch_id: EpochId) -> Self {
        Self {
            epoch_id,
            flags:            EpochFlags::default(),
            modified_objects: Vec::new(),
            deleted_objects:  Vec::new(),
        }
    }

    /// Ajoute un objet modifié (règle OOM-02 : try_reserve avant push).
    pub fn add_modified(
        &mut self,
        object_id:  ObjectId,
        disk_offset: DiskOffset,
        entry_flags: u8,
    ) -> ExofsResult<()> {
        self.modified_objects
            .try_reserve(1)
            .map_err(|_| ExofsError::NoMemory)?;
        let mut oid_bytes = [0u8; 32];
        oid_bytes.copy_from_slice(&object_id.0);
        self.modified_objects.push(EpochRootEntry {
            object_id:   oid_bytes,
            disk_offset: disk_offset.0,
            entry_flags,
            _pad:        [0u8; 7],
        });
        Ok(())
    }

    /// Ajoute un objet supprimé (règle OOM-02).
    pub fn add_deleted(&mut self, object_id: ObjectId) -> ExofsResult<()> {
        self.deleted_objects
            .try_reserve(1)
            .map_err(|_| ExofsError::NoMemory)?;
        self.deleted_objects.push(object_id);
        if !self.flags.contains(EpochFlags::HAS_DELETIONS) {
            self.flags.set(EpochFlags::HAS_DELETIONS);
        }
        Ok(())
    }

    /// Nombre total d'entrées (modifiées + supprimées).
    #[inline]
    pub fn total_entries(&self) -> usize {
        self.modified_objects.len() + self.deleted_objects.len()
    }

    /// Retourne vrai si l'epoch est plein (> EPOCH_MAX_OBJECTS).
    #[inline]
    pub fn is_full(&self) -> bool {
        self.total_entries() >= crate::fs::exofs::core::EPOCH_MAX_OBJECTS
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Vérification d'une page EpochRoot lue depuis disque
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie une page EpochRoot lue depuis disque.
///
/// RÈGLE CHAIN-01 : magic + checksum vérifiés AVANT lecture des entrées.
/// Retourne Err si la page est corrompue.
pub fn verify_epoch_root_page(page_data: &[u8]) -> ExofsResult<()> {
    if page_data.len() < size_of::<EpochRootPageHeader>() {
        return Err(ExofsError::CorruptedStructure);
    }
    // Lecture du magic (4 octets) EN PREMIER.
    let magic = u32::from_le_bytes([page_data[0], page_data[1], page_data[2], page_data[3]]);
    if magic != EPOCH_ROOT_MAGIC {
        return Err(ExofsError::InvalidMagic);
    }
    // Vérification du checksum : sur (page_data.len() - 32) premiers octets.
    let body_len = page_data.len().saturating_sub(32);
    let expected = blake3_hash(&page_data[..body_len]);
    let stored = &page_data[body_len..];
    if stored.len() != 32 {
        return Err(ExofsError::CorruptedStructure);
    }
    let mut acc: u8 = 0;
    for i in 0..32 {
        acc |= expected[i] ^ stored[i];
    }
    if acc != 0 {
        return Err(ExofsError::ChecksumMismatch);
    }
    Ok(())
}
