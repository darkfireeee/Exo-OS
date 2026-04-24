// kernel/src/fs/exofs/epoch/epoch_delta.rs
//
// =============================================================================
// Delta d'un Epoch — liste ordonnée des mutations d'un epoch
// Ring 0 · no_std · Exo-OS
// =============================================================================
//
// L'EpochDelta est la vue applicative in-memory des modifications d'un epoch.
// Il est construit AVANT la sérialisation en EpochRoot et sert de source
// unique de vérité pour le commit.
//
// RÈGLE EPOCH-05 : Si delta.len() >= EPOCH_MAX_OBJECTS → commit anticipé.
// RÈGLE OOM-02   : try_reserve(1)? avant push().
// RÈGLE ARITH-02 : checked_add/saturating_* pour toute arithmétique.
// RÈGLE RECUR-01 : itération strictement itérative.

use core::fmt;

use alloc::vec::Vec;

use crate::fs::exofs::core::flags::ObjectFlags;
use crate::fs::exofs::core::{
    BlobId, DiskOffset, EpochId, ExofsError, ExofsResult, ObjectId, EPOCH_MAX_OBJECTS,
};

// =============================================================================
// DeltaOpKind — type de mutation
// =============================================================================

/// Type d'une mutation dans l'epoch delta.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum DeltaOpKind {
    /// Création d'un nouvel objet.
    Create = 0,
    /// Nouvelle version d'un objet Class2 (CoW).
    CowWrite = 1,
    /// Mise à jour des métadonnées uniquement (droits, flags...).
    MetaUpdate = 2,
    /// Alias de déduplication (pointe sur un blob existant).
    DedupAlias = 3,
    /// Suppression logique définitive.
    Delete = 4,
}

impl DeltaOpKind {
    /// Vrai si cette opération modifie les données (pas metadata seul).
    #[inline]
    pub fn modifies_data(self) -> bool {
        matches!(self, Self::Create | Self::CowWrite | Self::DedupAlias)
    }

    /// Vrai si cette opération est une suppression.
    #[inline]
    pub fn is_delete(self) -> bool {
        self == Self::Delete
    }
}

impl fmt::Display for DeltaOpKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Create => write!(f, "CREATE"),
            Self::CowWrite => write!(f, "COW"),
            Self::MetaUpdate => write!(f, "META"),
            Self::DedupAlias => write!(f, "DEDUP"),
            Self::Delete => write!(f, "DELETE"),
        }
    }
}

// =============================================================================
// DeltaEntry — une entrée dans le delta
// =============================================================================

/// Une entrée dans le delta d'un epoch.
#[derive(Clone, Debug)]
pub struct DeltaEntry {
    /// ObjectId affecté.
    pub object_id: ObjectId,
    /// Type d'opération.
    pub op: DeltaOpKind,
    /// Offset disque de la nouvelle version (0 si Delete).
    pub disk_offset: DiskOffset,
    /// BlobId si l'objet a un contenu (None si Delete ou MetaUpdate).
    pub blob_id: Option<BlobId>,
    /// Flags de l'objet après cette opération.
    pub flags: ObjectFlags,
    /// Taille en octets du blob (0 si Delete ou MetaUpdate).
    pub size_bytes: u64,
}

impl DeltaEntry {
    /// Crée une entrée Create.
    pub fn new_create(
        object_id: ObjectId,
        disk_offset: DiskOffset,
        blob_id: BlobId,
        flags: ObjectFlags,
        size_bytes: u64,
    ) -> Self {
        Self {
            object_id,
            op: DeltaOpKind::Create,
            disk_offset,
            blob_id: Some(blob_id),
            flags,
            size_bytes,
        }
    }

    /// Crée une entrée Delete.
    pub fn new_delete(object_id: ObjectId) -> Self {
        Self {
            object_id,
            op: DeltaOpKind::Delete,
            disk_offset: DiskOffset(0),
            blob_id: None,
            flags: ObjectFlags::default(),
            size_bytes: 0,
        }
    }

    /// Crée une entrée CowWrite.
    pub fn new_cow_write(
        object_id: ObjectId,
        disk_offset: DiskOffset,
        blob_id: BlobId,
        flags: ObjectFlags,
        size_bytes: u64,
    ) -> Self {
        Self {
            object_id,
            op: DeltaOpKind::CowWrite,
            disk_offset,
            blob_id: Some(blob_id),
            flags,
            size_bytes,
        }
    }

    /// Crée une entrée MetaUpdate.
    pub fn new_meta_update(object_id: ObjectId, flags: ObjectFlags) -> Self {
        Self {
            object_id,
            op: DeltaOpKind::MetaUpdate,
            disk_offset: DiskOffset(0),
            blob_id: None,
            flags,
            size_bytes: 0,
        }
    }
}

impl fmt::Display for DeltaEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "DeltaEntry{{ op={} oid={:02x}{:02x}... offset={} size={} }}",
            self.op, self.object_id.0[0], self.object_id.0[1], self.disk_offset.0, self.size_bytes,
        )
    }
}

// =============================================================================
// DeltaSortOrder — critères de tri
// =============================================================================

/// Critère de tri des entrées du delta.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DeltaSortOrder {
    /// Tri par ObjectId (lexicographique).
    ByObjectId,
    /// Tri par type d'opération (Create < CowWrite < MetaUpdate < DedupAlias < Delete).
    ByOpKind,
    /// Tri par offset disque (optimal pour les écritures séquentielles).
    ByDiskOffset,
}

// =============================================================================
// EpochDelta — accumulateur d'opérations
// =============================================================================

/// Accumulateur de mutations pour l'epoch courant.
pub struct EpochDelta {
    /// Epoch auquel appartient ce delta.
    pub epoch_id: EpochId,
    /// Liste ordonnée des entrées.
    pub entries: Vec<DeltaEntry>,
    /// Vrai si au moins une suppression a été enregistrée.
    pub has_deletes: bool,
    /// Vrai si on a des créations.
    pub has_creates: bool,
    /// Total d'octets de données dans ce delta.
    pub total_bytes: u64,
}

impl EpochDelta {
    /// Crée un delta vide pour l'epoch donné.
    pub fn new(epoch_id: EpochId) -> Self {
        Self {
            epoch_id,
            entries: Vec::new(),
            has_deletes: false,
            has_creates: false,
            total_bytes: 0,
        }
    }

    // ── Ajout d'entrées ────────────────────────────────────────────────────

    /// Ajoute une entrée dans le delta.
    ///
    /// RÈGLE OOM-02 : try_reserve(1)? avant push().
    /// RÈGLE EPOCH-05 : retourne Err(EpochFull) si >= EPOCH_MAX_OBJECTS.
    pub fn push(&mut self, entry: DeltaEntry) -> ExofsResult<()> {
        if self.entries.len() >= EPOCH_MAX_OBJECTS {
            return Err(ExofsError::EpochFull);
        }
        self.entries
            .try_reserve(1)
            .map_err(|_| ExofsError::NoMemory)?;
        match entry.op {
            DeltaOpKind::Delete => {
                self.has_deletes = true;
            }
            DeltaOpKind::Create => {
                self.has_creates = true;
            }
            _ => {}
        }
        self.total_bytes = self.total_bytes.saturating_add(entry.size_bytes);
        self.entries.push(entry);
        Ok(())
    }

    /// Ajoute toutes les entrées d'un autre delta dans celui-ci.
    ///
    /// RÈGLE OOM-02 : try_reserve(other.len()) avant merge.
    /// Retourne Err(EpochFull) si le résultat dépasserait EPOCH_MAX_OBJECTS.
    pub fn merge_from(&mut self, other: &EpochDelta) -> ExofsResult<()> {
        let new_len = self
            .entries
            .len()
            .checked_add(other.entries.len())
            .ok_or(ExofsError::OffsetOverflow)?;
        if new_len > EPOCH_MAX_OBJECTS {
            return Err(ExofsError::EpochFull);
        }
        self.entries
            .try_reserve(other.entries.len())
            .map_err(|_| ExofsError::NoMemory)?;
        // RÈGLE RECUR-01 : itération itérative.
        for entry in &other.entries {
            match entry.op {
                DeltaOpKind::Delete => {
                    self.has_deletes = true;
                }
                DeltaOpKind::Create => {
                    self.has_creates = true;
                }
                _ => {}
            }
            self.total_bytes = self.total_bytes.saturating_add(entry.size_bytes);
            self.entries.push(entry.clone());
        }
        Ok(())
    }

    // ── Navigation ─────────────────────────────────────────────────────────

    /// Cherche une entrée par ObjectId.
    ///
    /// RÈGLE RECUR-01 : recherche linéaire itérative.
    pub fn find_object(&self, oid: &ObjectId) -> Option<&DeltaEntry> {
        for entry in &self.entries {
            if entry.object_id.0 == oid.0 {
                return Some(entry);
            }
        }
        None
    }

    /// Vrai si le delta contient une entrée pour cet ObjectId.
    #[inline]
    pub fn contains_object(&self, oid: &ObjectId) -> bool {
        self.find_object(oid).is_some()
    }

    /// Vrai si le delta contient une suppression pour cet ObjectId.
    pub fn has_delete_for(&self, oid: &ObjectId) -> bool {
        for entry in &self.entries {
            if entry.object_id.0 == oid.0 && entry.op.is_delete() {
                return true;
            }
        }
        false
    }

    /// Itère sur les suppressions uniquement.
    pub fn deletions(&self) -> impl Iterator<Item = &DeltaEntry> {
        self.entries.iter().filter(|e| e.op.is_delete())
    }

    /// Itère sur les créations/modifications uniquement.
    pub fn mutations(&self) -> impl Iterator<Item = &DeltaEntry> {
        self.entries.iter().filter(|e| !e.op.is_delete())
    }

    /// Compte les entrées par type d'opération.
    pub fn count_by_kind(&self) -> DeltaOpCounts {
        let mut c = DeltaOpCounts::default();
        for entry in &self.entries {
            match entry.op {
                DeltaOpKind::Create => c.creates += 1,
                DeltaOpKind::CowWrite => c.cow_writes += 1,
                DeltaOpKind::MetaUpdate => c.meta_updates += 1,
                DeltaOpKind::DedupAlias => c.dedup_aliases += 1,
                DeltaOpKind::Delete => c.deletes += 1,
            }
        }
        c
    }

    // ── Tri ────────────────────────────────────────────────────────────────

    /// Trie les entrées selon le critère donné.
    ///
    /// RÈGLE RECUR-01 : sort_unstable interne (pas de récursion utilisateur).
    pub fn sort_by(&mut self, order: DeltaSortOrder) {
        match order {
            DeltaSortOrder::ByObjectId => {
                self.entries
                    .sort_unstable_by(|a, b| a.object_id.0.cmp(&b.object_id.0));
            }
            DeltaSortOrder::ByOpKind => {
                self.entries.sort_unstable_by_key(|e| e.op);
            }
            DeltaSortOrder::ByDiskOffset => {
                self.entries.sort_unstable_by_key(|e| e.disk_offset.0);
            }
        }
    }

    // ── Découpage ──────────────────────────────────────────────────────────

    /// Découpe ce delta en deux si len() > max_entries.
    ///
    /// Retourne `Some(second_half)` si le split s'est produit, `None` sinon.
    /// RÈGLE OOM-02 : try_reserve avant construction du second half.
    pub fn split(&mut self, max_entries: usize) -> ExofsResult<Option<EpochDelta>> {
        if self.entries.len() <= max_entries {
            return Ok(None);
        }
        let tail = self.entries.split_off(max_entries);
        let mut other = EpochDelta::new(self.epoch_id);
        other
            .entries
            .try_reserve(tail.len())
            .map_err(|_| ExofsError::NoMemory)?;
        for entry in tail {
            match entry.op {
                DeltaOpKind::Delete => {
                    other.has_deletes = true;
                }
                DeltaOpKind::Create => {
                    other.has_creates = true;
                }
                _ => {}
            }
            other.total_bytes = other.total_bytes.saturating_add(entry.size_bytes);
            other.entries.push(entry);
        }
        // Recalcule les flags du self tronqué.
        self.has_deletes = self.entries.iter().any(|e| e.op.is_delete());
        self.has_creates = self.entries.iter().any(|e| e.op == DeltaOpKind::Create);
        self.total_bytes = self
            .entries
            .iter()
            .fold(0u64, |acc, e| acc.saturating_add(e.size_bytes));
        Ok(Some(other))
    }

    // ── Détection de conflits ──────────────────────────────────────────────

    /// Vrai si ce delta entre en conflit avec un autre (même ObjectId = conflit).
    ///
    /// RÈGLE RECUR-01 : double boucle itérative.
    pub fn has_conflict_with(&self, other: &EpochDelta) -> bool {
        for a in &self.entries {
            for b in &other.entries {
                if a.object_id.0 == b.object_id.0 {
                    return true;
                }
            }
        }
        false
    }

    // ── Stats ─────────────────────────────────────────────────────────────

    /// Retourne les statistiques du delta.
    pub fn stats(&self) -> DeltaStats {
        DeltaStats {
            epoch_id: self.epoch_id,
            entry_count: self.entries.len() as u64,
            total_bytes: self.total_bytes,
            ops: self.count_by_kind(),
            fill_pct: (self.entries.len() as u64)
                .saturating_mul(100)
                .checked_div(EPOCH_MAX_OBJECTS as u64)
                .unwrap_or(0),
        }
    }

    // ── Accesseurs simples ────────────────────────────────────────────────

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

    /// Vrai si le delta est plein.
    #[inline]
    pub fn is_full(&self) -> bool {
        self.entries.len() >= EPOCH_MAX_OBJECTS
    }

    /// Vrai si le delta est à mi-capacité (→ commit préemptif recommandé).
    #[inline]
    pub fn is_half_full(&self) -> bool {
        self.entries.len() >= EPOCH_MAX_OBJECTS / 2
    }
}

// =============================================================================
// DeltaOpCounts — compteurs par type d'opération
// =============================================================================

/// Compteurs des opérations du delta par type.
#[derive(Copy, Clone, Debug, Default)]
pub struct DeltaOpCounts {
    pub creates: usize,
    pub cow_writes: usize,
    pub meta_updates: usize,
    pub dedup_aliases: usize,
    pub deletes: usize,
}

impl DeltaOpCounts {
    /// Total d'opérations (toutes catégories).
    #[inline]
    pub fn total(&self) -> usize {
        self.creates
            .saturating_add(self.cow_writes)
            .saturating_add(self.meta_updates)
            .saturating_add(self.dedup_aliases)
            .saturating_add(self.deletes)
    }
}

impl fmt::Display for DeltaOpCounts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ops[create={} cow={} meta={} dedup={} del={}]",
            self.creates, self.cow_writes, self.meta_updates, self.dedup_aliases, self.deletes,
        )
    }
}

// =============================================================================
// DeltaStats — snapshot des métriques
// =============================================================================

/// Statistiques d'un delta (snapshot non-atomique pour diagnostic).
#[derive(Clone, Debug)]
pub struct DeltaStats {
    pub epoch_id: EpochId,
    pub entry_count: u64,
    pub total_bytes: u64,
    pub ops: DeltaOpCounts,
    /// Remplissage en pourcents (0..100).
    pub fill_pct: u64,
}

impl fmt::Display for DeltaStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "DeltaStats{{ epoch={} entries={} bytes={} fill={}% {} }}",
            self.epoch_id.0, self.entry_count, self.total_bytes, self.fill_pct, self.ops,
        )
    }
}
