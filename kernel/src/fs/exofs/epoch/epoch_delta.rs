// kernel/src/fs/exofs/epoch/epoch_delta.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Delta d'un Epoch — liste ordonnée des mutations d'un epoch
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// L'EpochDelta est la vue applicative in-memory des modifications d'un epoch.
// Il est construit AVANT la sérialisation en EpochRoot et sert de source
// unique de vérité pour le commit.
//
// RÈGLE EPOCH-05 : Si delta.len() >= EPOCH_MAX_OBJECTS → commit anticipé.
// RÈGLE OOM-02   : try_reserve(1)? avant push().

use alloc::vec::Vec;

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, EpochId, DiskOffset, ObjectId, BlobId,
    EPOCH_MAX_OBJECTS,
};
use crate::fs::exofs::core::flags::ObjectFlags;

// ─────────────────────────────────────────────────────────────────────────────
// DeltaOperation — type de mutation
// ─────────────────────────────────────────────────────────────────────────────

/// Type d'une mutation dans l'epoch delta.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DeltaOpKind {
    /// Nouvelle version d'un objet Class2 (CoW).
    CowWrite         = 0,
    /// Création d'un nouvel objet.
    Create           = 1,
    /// Suppression logique définitive.
    Delete           = 2,
    /// Mise à jour des métadonnées uniquement (droits, flags...).
    MetaUpdate       = 3,
    /// Alias de déduplication (pointe sur un blob existant).
    DedupAlias       = 4,
}

/// Une entrée dans le delta d'un epoch.
#[derive(Clone, Debug)]
pub struct DeltaEntry {
    /// ObjectId affecté.
    pub object_id:    ObjectId,
    /// Type d'opération.
    pub op:           DeltaOpKind,
    /// Offset disque de la nouvelle version (0 si Delete).
    pub disk_offset:  DiskOffset,
    /// BlobId si l'objet a un contenu (None si Delete ou MetaUpdate).
    pub blob_id:      Option<BlobId>,
    /// Flags de l'objet après cette opération.
    pub flags:        ObjectFlags,
}

// ─────────────────────────────────────────────────────────────────────────────
// EpochDelta — accumulateur d'opérations
// ─────────────────────────────────────────────────────────────────────────────

/// Accumulateur de mutations pour l'epoch courant.
pub struct EpochDelta {
    /// Epoch auquel appartient ce delta.
    pub epoch_id:  EpochId,
    /// Liste ordonnée des entrées.
    pub entries:   Vec<DeltaEntry>,
    /// Vrai si au moins une suppression a été enregistrée.
    pub has_deletes: bool,
}

impl EpochDelta {
    /// Crée un delta vide pour l'epoch donné.
    pub fn new(epoch_id: EpochId) -> Self {
        Self {
            epoch_id,
            entries:     Vec::new(),
            has_deletes: false,
        }
    }

    /// Ajoute une entrée dans le delta.
    ///
    /// RÈGLE OOM-02 : try_reserve(1)? avant push().
    /// RÈGLE EPOCH-05 : retourne Err(EpochFull) si > EPOCH_MAX_OBJECTS.
    pub fn push(&mut self, entry: DeltaEntry) -> ExofsResult<()> {
        if self.entries.len() >= EPOCH_MAX_OBJECTS {
            return Err(ExofsError::EpochFull);
        }
        self.entries.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        if entry.op == DeltaOpKind::Delete {
            self.has_deletes = true;
        }
        self.entries.push(entry);
        Ok(())
    }

    /// Vrai si le delta est plein.
    #[inline]
    pub fn is_full(&self) -> bool {
        self.entries.len() >= EPOCH_MAX_OBJECTS
    }

    /// Nombre d'entrées dans le delta.
    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Vrai si le delta est vide.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Itère sur les suppressions uniquement.
    pub fn deletions(&self) -> impl Iterator<Item = &DeltaEntry> {
        self.entries.iter().filter(|e| e.op == DeltaOpKind::Delete)
    }

    /// Itère sur les créations/modifications uniquement.
    pub fn mutations(&self) -> impl Iterator<Item = &DeltaEntry> {
        self.entries.iter().filter(|e| e.op != DeltaOpKind::Delete)
    }
}
