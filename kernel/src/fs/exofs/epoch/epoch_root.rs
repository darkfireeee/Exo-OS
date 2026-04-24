// kernel/src/fs/exofs/epoch/epoch_root.rs
//
// =============================================================================
// EpochRoot — liste des objets modifiés dans un Epoch (variable, multi-pages)
// Ring 0 · no_std · Exo-OS
// =============================================================================
//
// RÈGLE CHAIN-01 : Chaque page chainée vérifie son propre magic + checksum AVANT lecture.
// RÈGLE EPOCH-07 : magic 0x45504F43 ("EPOC") dans chaque page chainée.
// RÈGLE OOM-02   : try_reserve(1)? avant push().
// RÈGLE ARITH-02 : checked_add/saturating_add pour toute arithmétique.
// RÈGLE RECUR-01 : itération strictement itérative (pas de récursion).

use core::fmt;
use core::mem::size_of;

use alloc::vec::Vec;

use crate::fs::exofs::core::flags::EpochFlags;
use crate::fs::exofs::core::{
    blake3_hash, DiskOffset, EpochId, ExofsError, ExofsResult, ObjectId, EPOCH_MAX_OBJECTS,
    EPOCH_ROOT_MAGIC,
};
use crate::fs::exofs::epoch::epoch_delta::{DeltaOpKind, EpochDelta};

// =============================================================================
// EpochRootPageHeader — en-tête de chaque page chainée (on-disk)
// =============================================================================

/// En-tête de chaque page de l'EpochRoot (on-disk).
///
/// Chaque page contient : header + liste d'EpochRootEntry + checksum final.
/// RÈGLE ONDISK-01 : #[repr(C, packed)] + types plain uniquement.
#[derive(Copy, Clone)]
#[repr(C, packed)]
pub struct EpochRootPageHeader {
    /// Magic EPOC : 0x45504F43.
    pub magic: u32,
    /// Version du format de page.
    pub version: u16,
    /// Flags de l'epoch.
    pub flags: u16,
    /// Identifiant de l'epoch.
    pub epoch_id: u64,
    /// Nombre d'entrées dans cette page.
    pub entry_count: u32,
    /// Index de cette page dans la chaîne (0-based).
    pub page_index: u32,
    /// Offset de la page suivante (0 = fin de chaîne).
    pub next_page: u64,
    /// Checksum Blake3 de cette page (header + entries).
    pub checksum: [u8; 32],
}

const _: () = assert!(
    size_of::<EpochRootPageHeader>() == 64,
    "EpochRootPageHeader doit etre 64 octets"
);

// =============================================================================
// EpochRootEntry — un objet modifié ou supprimé dans l'epoch (on-disk)
// =============================================================================

/// Entrée d'un objet dans l'EpochRoot.
/// RÈGLE ONDISK-01 : #[repr(C, packed)], types plain uniquement.
#[derive(Copy, Clone, Debug)]
#[repr(C, packed)]
pub struct EpochRootEntry {
    /// ObjectId de l'objet modifié.
    pub object_id: [u8; 32],
    /// Offset disque de sa nouvelle version.
    pub disk_offset: u64,
    /// Flags (bit 0 = supprimé, bit 1 = créé, bit 2 = modifié, bit 3 = meta).
    pub entry_flags: u8,
    /// _pad pour alignement.
    pub _pad: [u8; 7],
}

const _: () = assert!(
    size_of::<EpochRootEntry>() == 48,
    "EpochRootEntry doit etre 48 octets"
);

impl EpochRootEntry {
    pub const FLAG_DELETED: u8 = 1 << 0;
    pub const FLAG_CREATED: u8 = 1 << 1;
    pub const FLAG_MODIFIED: u8 = 1 << 2;
    pub const FLAG_META: u8 = 1 << 3;

    /// Retourne vrai si cette entrée est une suppression.
    #[inline]
    pub fn is_deleted(self) -> bool {
        self.entry_flags & Self::FLAG_DELETED != 0
    }

    /// Retourne vrai si cette entrée est une création.
    #[inline]
    pub fn is_created(self) -> bool {
        self.entry_flags & Self::FLAG_CREATED != 0
    }

    /// Retourne vrai si cette entrée est une modification de données.
    #[inline]
    pub fn is_modified(self) -> bool {
        self.entry_flags & Self::FLAG_MODIFIED != 0
    }

    /// Retourne vrai si cette entrée est une mise à jour de métadonnées.
    #[inline]
    pub fn is_meta(self) -> bool {
        self.entry_flags & Self::FLAG_META != 0
    }

    /// Construit une entrée depuis ses composants.
    pub fn new(object_id: ObjectId, disk_offset: DiskOffset, entry_flags: u8) -> Self {
        Self {
            object_id: object_id.0,
            disk_offset: disk_offset.0,
            entry_flags,
            _pad: [0u8; 7],
        }
    }
}

impl fmt::Debug for EpochRootPageHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // SAFETY: magic est packed u32, lecture via copy.
        let magic = self.magic;
        let epoch_id = self.epoch_id;
        let entry_count = self.entry_count;
        let page_index = self.page_index;
        write!(
            f,
            "EpochRootPageHeader{{ magic={:#010x} epoch={} idx={} entries={} }}",
            magic, epoch_id, page_index, entry_count,
        )
    }
}

// =============================================================================
// EpochRootInMemory — accumule les modifications avant écriture
// =============================================================================

/// EpochRoot en mémoire vive — accumule les entrées de l'epoch courant.
pub struct EpochRootInMemory {
    /// Identifiant de l'epoch courant.
    pub epoch_id: EpochId,
    /// Flags de l'epoch.
    pub flags: EpochFlags,
    /// Objets modifiés ou créés dans cet epoch.
    pub modified_objects: Vec<EpochRootEntry>,
    /// Objets supprimés dans cet epoch (stockés séparément pour accès rapide GC).
    pub deleted_objects: Vec<ObjectId>,
}

impl EpochRootInMemory {
    /// Crée un EpochRoot vide pour l'epoch donné.
    pub fn new(epoch_id: EpochId) -> Self {
        Self {
            epoch_id,
            flags: EpochFlags::default(),
            modified_objects: Vec::new(),
            deleted_objects: Vec::new(),
        }
    }

    // ── Ajout d'entrées ─────────────────────────────────────────────────────

    /// Ajoute un objet modifié/créé.
    ///
    /// RÈGLE OOM-02 : try_reserve(1)? avant push().
    /// RÈGLE EPOCH-05 : limit EPOCH_MAX_OBJECTS.
    pub fn add_modified(
        &mut self,
        object_id: ObjectId,
        disk_offset: DiskOffset,
        entry_flags: u8,
    ) -> ExofsResult<()> {
        if self.total_entries() >= EPOCH_MAX_OBJECTS {
            return Err(ExofsError::EpochFull);
        }
        self.modified_objects
            .try_reserve(1)
            .map_err(|_| ExofsError::NoMemory)?;
        self.modified_objects
            .push(EpochRootEntry::new(object_id, disk_offset, entry_flags));
        Ok(())
    }

    /// Ajoute un objet supprimé.
    ///
    /// RÈGLE OOM-02 : try_reserve(1)? avant push().
    pub fn add_deleted(&mut self, object_id: ObjectId) -> ExofsResult<()> {
        if self.total_entries() >= EPOCH_MAX_OBJECTS {
            return Err(ExofsError::EpochFull);
        }
        self.deleted_objects
            .try_reserve(1)
            .map_err(|_| ExofsError::NoMemory)?;
        self.deleted_objects.push(object_id);
        self.flags.set(EpochFlags::HAS_DELETIONS);
        Ok(())
    }

    /// Intègre les entrées d'un EpochDelta dans cet EpochRoot.
    ///
    /// RÈGLE RECUR-01 : boucle itérative.
    pub fn add_from_delta(&mut self, delta: &EpochDelta) -> ExofsResult<()> {
        let additions = delta.entries.len();
        if self
            .total_entries()
            .checked_add(additions)
            .map(|t| t > EPOCH_MAX_OBJECTS)
            .unwrap_or(true)
        {
            return Err(ExofsError::EpochFull);
        }
        for entry in &delta.entries {
            match entry.op {
                DeltaOpKind::Delete => {
                    self.add_deleted(entry.object_id)?;
                }
                DeltaOpKind::Create => {
                    self.add_modified(
                        entry.object_id,
                        entry.disk_offset,
                        EpochRootEntry::FLAG_CREATED,
                    )?;
                }
                DeltaOpKind::CowWrite => {
                    self.add_modified(
                        entry.object_id,
                        entry.disk_offset,
                        EpochRootEntry::FLAG_MODIFIED,
                    )?;
                }
                DeltaOpKind::MetaUpdate => {
                    self.add_modified(
                        entry.object_id,
                        entry.disk_offset,
                        EpochRootEntry::FLAG_META,
                    )?;
                }
                DeltaOpKind::DedupAlias => {
                    self.add_modified(
                        entry.object_id,
                        entry.disk_offset,
                        EpochRootEntry::FLAG_MODIFIED,
                    )?;
                }
            }
        }
        Ok(())
    }

    // ── Recherche ───────────────────────────────────────────────────────────

    /// Cherche une entrée modifiée par ObjectId.
    ///
    /// RÈGLE RECUR-01 : recherche linéaire itérative.
    pub fn find_modified(&self, oid: &ObjectId) -> Option<&EpochRootEntry> {
        for entry in &self.modified_objects {
            if &entry.object_id == &oid.0 {
                return Some(entry);
            }
        }
        None
    }

    /// Vrai si l'ObjectId a été supprimé dans cet epoch.
    pub fn is_deleted(&self, oid: &ObjectId) -> bool {
        for del in &self.deleted_objects {
            if del.0 == oid.0 {
                return true;
            }
        }
        false
    }

    /// Vrai si l'ObjectId est présent dans cet epoch (modifié ou supprimé).
    #[inline]
    pub fn contains(&self, oid: &ObjectId) -> bool {
        self.find_modified(oid).is_some() || self.is_deleted(oid)
    }

    // ── Métriques ───────────────────────────────────────────────────────────

    /// Nombre total d'entrées (modifiées + supprimées).
    #[inline]
    pub fn total_entries(&self) -> usize {
        self.modified_objects.len() + self.deleted_objects.len()
    }

    /// Retourne vrai si l'epoch est plein (>= EPOCH_MAX_OBJECTS).
    #[inline]
    pub fn is_full(&self) -> bool {
        self.total_entries() >= EPOCH_MAX_OBJECTS
    }

    /// Retourne les statistiques de cet EpochRoot.
    pub fn stats(&self) -> EpochRootStats {
        let mut created = 0u32;
        let mut modified = 0u32;
        let mut meta = 0u32;
        for e in &self.modified_objects {
            if e.is_created() {
                created += 1;
            }
            if e.is_modified() {
                modified += 1;
            }
            if e.is_meta() {
                meta += 1;
            }
        }
        EpochRootStats {
            epoch_id: self.epoch_id,
            modified_count: self.modified_objects.len() as u32,
            deleted_count: self.deleted_objects.len() as u32,
            created_count: created,
            data_edits: modified,
            meta_edits: meta,
            fill_pct: (self.total_entries() as u64)
                .saturating_mul(100)
                .checked_div(EPOCH_MAX_OBJECTS as u64)
                .unwrap_or(0),
        }
    }
}

// =============================================================================
// EpochRootStats — métriques d'un EpochRoot
// =============================================================================

/// Statistiques d'un EpochRoot in-memory.
#[derive(Copy, Clone, Debug)]
pub struct EpochRootStats {
    pub epoch_id: EpochId,
    pub modified_count: u32,
    pub deleted_count: u32,
    pub created_count: u32,
    pub data_edits: u32,
    pub meta_edits: u32,
    pub fill_pct: u64,
}

impl fmt::Display for EpochRootStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "EpochRoot[epoch={} mod={} del={} create={} data={} meta={} fill={}%]",
            self.epoch_id.0,
            self.modified_count,
            self.deleted_count,
            self.created_count,
            self.data_edits,
            self.meta_edits,
            self.fill_pct,
        )
    }
}

// =============================================================================
// EpochRootBuilder — builder pattern pour la construction de l'EpochRoot
// =============================================================================

/// Builder pour construire un EpochRootInMemory de façon chainée.
pub struct EpochRootBuilder {
    inner: EpochRootInMemory,
}

impl EpochRootBuilder {
    /// Instancie le builder pour l'epoch donné.
    pub fn new(epoch_id: EpochId) -> Self {
        Self {
            inner: EpochRootInMemory::new(epoch_id),
        }
    }

    /// Définit les flags de l'epoch.
    pub fn with_flags(mut self, flags: EpochFlags) -> Self {
        self.inner.flags = flags;
        self
    }

    /// Ajoute un objet modifié.
    pub fn add_modified(
        mut self,
        oid: ObjectId,
        disk_offset: DiskOffset,
        flags: u8,
    ) -> ExofsResult<Self> {
        self.inner.add_modified(oid, disk_offset, flags)?;
        Ok(self)
    }

    /// Ajoute un objet supprimé.
    pub fn add_deleted(mut self, oid: ObjectId) -> ExofsResult<Self> {
        self.inner.add_deleted(oid)?;
        Ok(self)
    }

    /// Intègre un EpochDelta complet.
    pub fn from_delta(mut self, delta: &EpochDelta) -> ExofsResult<Self> {
        self.inner.add_from_delta(delta)?;
        Ok(self)
    }

    /// Finalise et retourne l'EpochRootInMemory.
    pub fn build(self) -> EpochRootInMemory {
        self.inner
    }
}

// =============================================================================
// Vérification d'une page EpochRoot lue depuis disque
// =============================================================================

/// Vérifie une page EpochRoot lue depuis disque.
///
/// RÈGLE CHAIN-01 : magic vérifié EN PREMIER, puis checksum.
/// RÈGLE V-08/V-13 : magic → checksum → payload, jamais l'inverse.
pub fn verify_epoch_root_page(page_data: &[u8]) -> ExofsResult<()> {
    if page_data.len() < size_of::<EpochRootPageHeader>() {
        return Err(ExofsError::CorruptedStructure);
    }
    // RÈGLE V-08 : magic FIRST.
    let magic = u32::from_le_bytes([page_data[0], page_data[1], page_data[2], page_data[3]]);
    if magic != EPOCH_ROOT_MAGIC {
        return Err(ExofsError::InvalidMagic);
    }
    // RÈGLE V-13 : checksum ensuite.
    let body_len = page_data.len().saturating_sub(32);
    let expected = blake3_hash(&page_data[..body_len]);
    let stored = &page_data[body_len..];
    if stored.len() != 32 {
        return Err(ExofsError::CorruptedStructure);
    }
    // RÈGLE SEC-08 : comparaison en temps constant.
    let mut acc: u8 = 0;
    for i in 0..32 {
        acc |= expected[i] ^ stored[i];
    }
    if acc != 0 {
        return Err(ExofsError::ChecksumMismatch);
    }
    Ok(())
}

/// Lit l'en-tête d'une page EpochRoot validée.
///
/// Précondition : `verify_epoch_root_page(page_data)` doit avoir réussi.
pub fn read_page_header(page_data: &[u8]) -> ExofsResult<EpochRootPageHeader> {
    if page_data.len() < size_of::<EpochRootPageHeader>() {
        return Err(ExofsError::CorruptedStructure);
    }
    // SAFETY: EpochRootPageHeader est #[repr(C, packed)], Copy, plain types.
    let hdr: EpochRootPageHeader =
        unsafe { core::ptr::read_unaligned(page_data.as_ptr() as *const EpochRootPageHeader) };
    Ok(hdr)
}

/// Retourne les entrées d'une page EpochRoot validée.
///
/// RÈGLE RECUR-01 : itération itérative.
pub fn read_page_entries(page_data: &[u8]) -> ExofsResult<Vec<EpochRootEntry>> {
    // Vérification préalable (RÈGLE CHAIN-01).
    verify_epoch_root_page(page_data)?;
    let hdr = read_page_header(page_data)?;
    let entry_count = hdr.entry_count as usize;
    let entry_size = size_of::<EpochRootEntry>();
    let hdr_size = size_of::<EpochRootPageHeader>();
    let max_entries = page_data.len().saturating_sub(hdr_size + 32) / entry_size;
    if entry_count > max_entries {
        return Err(ExofsError::CorruptedStructure);
    }
    let mut result: Vec<EpochRootEntry> = Vec::new();
    result
        .try_reserve(entry_count)
        .map_err(|_| ExofsError::NoMemory)?;
    let mut offset = hdr_size;
    for _ in 0..entry_count {
        // SAFETY: EpochRootEntry est #[repr(C, packed)], taille 48, Copy.
        let entry: EpochRootEntry = unsafe {
            core::ptr::read_unaligned(page_data[offset..].as_ptr() as *const EpochRootEntry)
        };
        result.push(entry);
        offset = offset.saturating_add(entry_size);
    }
    Ok(result)
}

// ─────────────────────────────────────────────────────────────────────────────
// EpochRootPageHeader — 80 octets, tête de chaque page chainée
// ─────────────────────────────────────────────────────────────────────────────
