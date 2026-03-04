// SPDX-License-Identifier: MIT
// ExoFS — object_kind/relation.rs
// RelationDescriptor — arêtes typées du graphe d'objets ExoFS.
//
// Règles :
//   ONDISK-01 : RelationEntryDisk #[repr(C, packed)]
//   OOM-02    : try_reserve avant chaque push
//   ARITH-02  : checked_add / saturating_* partout
//   RECUR-01  : détection de cycle itérative (DFS itératif)

#![allow(dead_code)]

use core::fmt;
use core::mem;
use alloc::vec::Vec;

use crate::fs::exofs::core::{
    ObjectId, EpochId, ExofsError, ExofsResult, blake3_hash,
};

// ── Constantes ──────────────────────────────────────────────────────────────────

/// Magic d'un RelationTableDisk.
pub const RELATION_TABLE_MAGIC: u32 = 0x4552_4C54; // "ERLT"

/// Version du format RelationTableDisk.
pub const RELATION_TABLE_VERSION: u8 = 1;

/// Nombre maximal de relations dans une table.
pub const RELATION_MAX_COUNT: usize = 256;

/// Longueur maximale d'un label de relation.
pub const RELATION_LABEL_LEN: usize = 32;

// ── RelationKind ───────────────────────────────────────────────────────────────

/// Type d'une relation entre deux objets ExoFS.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum RelationKind {
    /// Contient (répertoire → entrée).
    Parent    = 0x01,
    /// Dépendance (A dépend de B pour s'exécuter).
    DependsOn = 0x02,
    /// Alias (A est interchangeable avec B).
    Alias     = 0x03,
    /// Symlink ResolveTo (A est un symlink pointant vers B).
    Symlink   = 0x04,
    /// Dérivé de (B est une version ultérieure de A).
    DerivedFrom = 0x05,
    /// Référence faible (A mentionne B sans dépendance forte).
    WeakRef   = 0x06,
    /// Appartenance (A est le propriétaire de B).
    Owns      = 0x07,
    /// Inconnu (valeur de sécurité).
    Unknown   = 0xFF,
}

impl RelationKind {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0x01 => Self::Parent,
            0x02 => Self::DependsOn,
            0x03 => Self::Alias,
            0x04 => Self::Symlink,
            0x05 => Self::DerivedFrom,
            0x06 => Self::WeakRef,
            0x07 => Self::Owns,
            _    => Self::Unknown,
        }
    }

    /// Vrai si ce type de relation peut créer un cycle (exclure Alias, WeakRef).
    pub fn can_form_cycle(&self) -> bool {
        matches!(
            self,
            Self::Parent | Self::DependsOn | Self::DerivedFrom | Self::Owns
        )
    }
}

impl fmt::Display for RelationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parent      => write!(f, "Parent"),
            Self::DependsOn   => write!(f, "DependsOn"),
            Self::Alias       => write!(f, "Alias"),
            Self::Symlink     => write!(f, "Symlink"),
            Self::DerivedFrom => write!(f, "DerivedFrom"),
            Self::WeakRef     => write!(f, "WeakRef"),
            Self::Owns        => write!(f, "Owns"),
            Self::Unknown     => write!(f, "Unknown"),
        }
    }
}

// ── RelationFlags ──────────────────────────────────────────────────────────────

/// Flags d'une relation.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RelationFlags(pub u16);

impl RelationFlags {
    /// Relation supprimée (tombstone).
    pub const DELETED:      Self = Self(1 << 0);
    /// Relation obligatoire (l'objet source ne peut exister sans la cible).
    pub const REQUIRED:     Self = Self(1 << 1);
    /// Relation en attente de confirmation (lazy).
    pub const PENDING:      Self = Self(1 << 2);
    /// Relation cassée (la cible n'existe plus).
    pub const BROKEN:       Self = Self(1 << 3);
    /// Relation héritée d'un snapshot parent.
    pub const INHERITED:    Self = Self(1 << 4);

    pub fn contains(&self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub fn empty() -> Self { Self(0) }
}

// ── RelationEntryDisk ──────────────────────────────────────────────────────────

/// Représentation on-disk d'une relation entre deux objets (128 octets).
///
/// Layout :
/// ```text
///   0.. 31  src_id       [u8;32]  — objet source
///  32.. 63  dst_id       [u8;32]  — objet destination
///  64.. 71  epoch_create u64
///  72.. 73  flags        u16
///  74       kind         u8
///  75       label_len    u8
///  76..107  label        [u8;32]  — étiquette optionnelle
/// 108..111  weight       u32      — poids/priorité (0 = non défini)
/// 112..127  checksum     [u8;16]  — Blake3(premières 112 B), tronqué
/// ```
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct RelationEntryDisk {
    pub src_id:       [u8; 32],
    pub dst_id:       [u8; 32],
    pub epoch_create: u64,
    pub flags:        u16,
    pub kind:         u8,
    pub label_len:    u8,
    pub label:        [u8; RELATION_LABEL_LEN],
    pub weight:       u32,
    pub checksum:     [u8; 16],
}

const _: () = assert!(
    mem::size_of::<RelationEntryDisk>() == 128,
    "RelationEntryDisk doit être 128 octets (ONDISK-01)"
);

impl RelationEntryDisk {
    pub fn compute_checksum(&self) -> [u8; 16] {
        let raw: &[u8; 128] =
            // SAFETY: pointeur calculé depuis une slice dont la longueur a été vérifiée.
            unsafe { &*(self as *const RelationEntryDisk as *const [u8; 128]) };
        let full = blake3_hash(&raw[..112]);
        let mut out = [0u8; 16];
        out.copy_from_slice(&full[..16]);
        out
    }

    pub fn verify(&self) -> ExofsResult<()> {
        let computed = self.compute_checksum();
        if { self.checksum } != computed {
            return Err(ExofsError::Corrupt);
        }
        if matches!(RelationKind::from_u8(self.kind), RelationKind::Unknown) {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(())
    }
}

impl fmt::Debug for RelationEntryDisk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "RelationEntryDisk {{ kind: {}, flags: {:#x} }}",
            { self.kind }, { self.flags },
        )
    }
}

// ── RelationDescriptor in-memory ─────────────────────────────────────────────

/// Descripteur in-memory d'une relation entre deux objets ExoFS.
#[derive(Clone, Debug)]
pub struct RelationDescriptor {
    /// Objet source.
    pub src:          ObjectId,
    /// Objet destination.
    pub dst:          ObjectId,
    /// Epoch de création.
    pub epoch_create: EpochId,
    /// Type de relation.
    pub kind:         RelationKind,
    /// Flags.
    pub flags:        RelationFlags,
    /// Étiquette optionnelle (max 32 octets, UTF-8).
    pub label:        [u8; RELATION_LABEL_LEN],
    pub label_len:    u8,
    /// Poids de la relation (0 = non défini).
    pub weight:       u32,
}

impl RelationDescriptor {
    // ── Constructeurs ──────────────────────────────────────────────────────────

    pub fn new(src: ObjectId, dst: ObjectId, kind: RelationKind, epoch: EpochId) -> Self {
        Self {
            src,
            dst,
            epoch_create: epoch,
            kind,
            flags:     RelationFlags::empty(),
            label:     [0u8; RELATION_LABEL_LEN],
            label_len: 0,
            weight:    0,
        }
    }

    /// Ajoute une étiquette.
    pub fn with_label(mut self, label: &[u8]) -> ExofsResult<Self> {
        if label.len() > RELATION_LABEL_LEN {
            return Err(ExofsError::Overflow);
        }
        self.label[..label.len()].copy_from_slice(label);
        self.label_len = label.len() as u8;
        Ok(self)
    }

    /// Définit le poids.
    pub fn with_weight(mut self, w: u32) -> Self {
        self.weight = w;
        self
    }

    /// Marque une relation obligatoire.
    pub fn required(mut self) -> Self {
        self.flags = RelationFlags(self.flags.0 | RelationFlags::REQUIRED.0);
        self
    }

    /// Reconstruit depuis on-disk (HDR-03 : verify() en premier).
    pub fn from_disk(d: &RelationEntryDisk) -> ExofsResult<Self> {
        d.verify()?;
        let kind = RelationKind::from_u8(d.kind);
        Ok(Self {
            src:          ObjectId(d.src_id),
            dst:          ObjectId(d.dst_id),
            epoch_create: EpochId(d.epoch_create),
            kind,
            flags:        RelationFlags(d.flags),
            label:        d.label,
            label_len:    d.label_len,
            weight:       d.weight,
        })
    }

    // ── Sérialisation ──────────────────────────────────────────────────────────

    pub fn to_disk(&self) -> RelationEntryDisk {
        let mut d = RelationEntryDisk {
            src_id:       self.src.0,
            dst_id:       self.dst.0,
            epoch_create: self.epoch_create.0,
            flags:        self.flags.0,
            kind:         self.kind as u8,
            label_len:    self.label_len,
            label:        self.label,
            weight:       self.weight,
            checksum:     [0u8; 16],
        };
        d.checksum = d.compute_checksum();
        d
    }

    // ── Requêtes ───────────────────────────────────────────────────────────────

    #[inline]
    pub fn is_deleted(&self) -> bool {
        self.flags.contains(RelationFlags::DELETED)
    }

    #[inline]
    pub fn is_broken(&self) -> bool {
        self.flags.contains(RelationFlags::BROKEN)
    }

    #[inline]
    pub fn label_bytes(&self) -> &[u8] {
        &self.label[..self.label_len as usize]
    }

    // ── Validation ────────────────────────────────────────────────────────────

    pub fn validate(&self) -> ExofsResult<()> {
        if matches!(self.kind, RelationKind::Unknown) {
            return Err(ExofsError::InvalidArgument);
        }
        if self.label_len as usize > RELATION_LABEL_LEN {
            return Err(ExofsError::Corrupt);
        }
        Ok(())
    }
}

impl fmt::Display for RelationDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Relation {{ src: {:02x?}..., dst: {:02x?}..., kind: {}, \
             flags: {:#x}, weight: {} }}",
            &self.src.0[..4], &self.dst.0[..4],
            self.kind, self.flags.0, self.weight,
        )
    }
}

// ── RelationTable ─────────────────────────────────────────────────────────────

/// Table de relations in-memory pour un objet ExoFS.
pub struct RelationTable {
    /// ObjectId de l'objet propriétaire.
    pub owner:        ObjectId,
    /// Epoch de dernière modification.
    pub epoch_modify: EpochId,
    /// Liste de relations.
    relations:        Vec<RelationDescriptor>,
}

impl RelationTable {
    // ── Constructeurs ──────────────────────────────────────────────────────────

    pub fn new(owner: ObjectId, epoch: EpochId) -> Self {
        Self {
            owner,
            epoch_modify: epoch,
            relations:    Vec::new(),
        }
    }

    // ── Opérations ────────────────────────────────────────────────────────────

    /// Insère une relation (OOM-02 : try_reserve).
    pub fn add(&mut self, rel: RelationDescriptor, now: EpochId) -> ExofsResult<()> {
        let active = self.relations.iter().filter(|r| !r.is_deleted()).count();
        if active >= RELATION_MAX_COUNT {
            return Err(ExofsError::NoSpace);
        }
        // Détection de doublon (src + dst + kind).
        for r in self.relations.iter() {
            if !r.is_deleted()
                && r.src == rel.src
                && r.dst == rel.dst
                && r.kind == rel.kind
            {
                return Err(ExofsError::InvalidArgument);
            }
        }
        self.relations.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.relations.push(rel);
        self.epoch_modify = now;
        Ok(())
    }

    /// Supprime une relation (tombstone).
    pub fn remove(
        &mut self,
        src:  &ObjectId,
        dst:  &ObjectId,
        kind: RelationKind,
        now:  EpochId,
    ) -> ExofsResult<()> {
        for r in self.relations.iter_mut() {
            if r.src == *src && r.dst == *dst && r.kind == kind && !r.is_deleted() {
                r.flags = RelationFlags(r.flags.0 | RelationFlags::DELETED.0);
                self.epoch_modify = now;
                return Ok(());
            }
        }
        Err(ExofsError::NotFound)
    }

    /// Retourne toutes les relations actives vers une destination.
    pub fn find_by_dst(&self, dst: &ObjectId) -> Vec<&RelationDescriptor> {
        let mut out = Vec::new();
        for r in self.relations.iter() {
            if !r.is_deleted() && r.dst == *dst {
                let _ = out.try_reserve(1);
                out.push(r);
            }
        }
        out
    }

    /// Retourne toutes les relations actives d'un type donné.
    pub fn find_by_kind(&self, kind: RelationKind) -> Vec<&RelationDescriptor> {
        let mut out = Vec::new();
        for r in self.relations.iter() {
            if !r.is_deleted() && r.kind == kind {
                let _ = out.try_reserve(1);
                out.push(r);
            }
        }
        out
    }

    /// Nombre de relations actives.
    pub fn len(&self) -> usize {
        self.relations.iter().filter(|r| !r.is_deleted()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    // ── Détection de cycle (DFS itératif, RECUR-01) ────────────────────────────

    /// Vrai si l'ajout de la relation `src → dst` de type `kind` créerait un cycle.
    ///
    /// Implémenté en DFS itératif (RECUR-01 : jamais récursif).
    pub fn would_create_cycle(
        &self,
        src:  &ObjectId,
        dst:  &ObjectId,
        kind: RelationKind,
    ) -> bool {
        if !kind.can_form_cycle() {
            return false;
        }
        // DFS itératif depuis `dst` : cherche si on peut atteindre `src`.
        let mut stack: Vec<ObjectId> = Vec::new();
        let mut visited: Vec<ObjectId> = Vec::new();
        let _ = stack.try_reserve(16);
        let _ = visited.try_reserve(16);
        let _ = stack.try_reserve(1);
        stack.push(*dst);

        while let Some(node) = stack.pop() {
            if node == *src {
                return true; // Cycle détecté.
            }
            // Éviter les revisites.
            if visited.contains(&node) {
                continue;
            }
            let _ = visited.try_reserve(1);
            visited.push(node);
            // Pousser les successeurs dans la pile.
            for r in self.relations.iter() {
                if !r.is_deleted() && r.src == node && r.kind == kind {
                    let _ = stack.try_reserve(1);
                    stack.push(r.dst);
                }
            }
        }
        false
    }

    // ── Sérialisation ──────────────────────────────────────────────────────────

    pub fn to_disk_vec(&self) -> ExofsResult<Vec<RelationEntryDisk>> {
        let active: Vec<&RelationDescriptor> = self
            .relations
            .iter()
            .filter(|r| !r.is_deleted())
            .collect();
        let mut out = Vec::new();
        out.try_reserve(active.len()).map_err(|_| ExofsError::NoMemory)?;
        for r in active {
            out.push(r.to_disk());
        }
        Ok(out)
    }

    pub fn from_disk_slice(
        entries: &[RelationEntryDisk],
        owner:   ObjectId,
        epoch:   EpochId,
    ) -> ExofsResult<Self> {
        if entries.len() > RELATION_MAX_COUNT {
            return Err(ExofsError::Overflow);
        }
        let mut table = Self::new(owner, epoch);
        table.relations.try_reserve(entries.len()).map_err(|_| ExofsError::NoMemory)?;
        for d in entries.iter() {
            let r = RelationDescriptor::from_disk(d)?;
            table.relations.push(r);
        }
        Ok(table)
    }

    // ── Validation ────────────────────────────────────────────────────────────

    pub fn validate(&self) -> ExofsResult<()> {
        for r in self.relations.iter() {
            if !r.is_deleted() {
                r.validate()?;
            }
        }
        Ok(())
    }
}

impl fmt::Display for RelationTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "RelationTable {{ owner: {:02x?}..., relations: {}, epoch: {} }}",
            &self.owner.0[..4], self.len(), self.epoch_modify.0,
        )
    }
}

impl fmt::Debug for RelationTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

// ── RelationStats ──────────────────────────────────────────────────────────────

/// Statistiques agrégées des tables de relations.
#[derive(Default, Debug)]
pub struct RelationStats {
    pub total_tables:    u64,
    pub total_relations: u64,
    pub tombstone_count: u64,
    pub broken_count:    u64,
    pub by_kind:         [u64; 8], // indexé par RelationKind as u8
}

impl RelationStats {
    pub fn new() -> Self { Self::default() }

    pub fn record(&mut self, table: &RelationTable) {
        self.total_tables = self.total_tables.saturating_add(1);
        for r in table.relations.iter() {
            self.total_relations = self.total_relations.saturating_add(1);
            if r.is_deleted() { self.tombstone_count = self.tombstone_count.saturating_add(1); }
            if r.is_broken()  { self.broken_count    = self.broken_count.saturating_add(1); }
            let idx = (r.kind as u8 as usize).min(7);
            self.by_kind[idx] = self.by_kind[idx].saturating_add(1);
        }
    }
}

impl fmt::Display for RelationStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "RelationStats {{ tables: {}, relations: {}, tombstones: {}, broken: {} }}",
            self.total_tables, self.total_relations,
            self.tombstone_count, self.broken_count,
        )
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entry_disk_size() {
        assert_eq!(mem::size_of::<RelationEntryDisk>(), 128);
    }

    #[test]
    fn test_relation_roundtrip() {
        let rel = RelationDescriptor::new(
            ObjectId([1;32]), ObjectId([2;32]),
            RelationKind::Parent, EpochId(1),
        );
        let disk = rel.to_disk();
        disk.verify().unwrap();
        let back = RelationDescriptor::from_disk(&disk).unwrap();
        assert!(matches!(back.kind, RelationKind::Parent));
    }

    #[test]
    fn test_cycle_detection() {
        let owner = ObjectId([0;32]);
        let a = ObjectId([1;32]);
        let b = ObjectId([2;32]);
        let c = ObjectId([3;32]);
        let mut table = RelationTable::new(owner, EpochId(1));
        // a → b
        table.add(RelationDescriptor::new(a, b, RelationKind::Parent, EpochId(1)), EpochId(1)).unwrap();
        // b → c
        table.add(RelationDescriptor::new(b, c, RelationKind::Parent, EpochId(1)), EpochId(1)).unwrap();
        // c → a serait un cycle
        assert!(table.would_create_cycle(&c, &a, RelationKind::Parent));
        assert!(!table.would_create_cycle(&c, &ObjectId([99;32]), RelationKind::Parent));
    }

    #[test]
    fn test_duplicate_detection() {
        let owner = ObjectId([0;32]);
        let a = ObjectId([1;32]);
        let b = ObjectId([2;32]);
        let mut table = RelationTable::new(owner, EpochId(1));
        table.add(RelationDescriptor::new(a, b, RelationKind::Alias, EpochId(1)), EpochId(1)).unwrap();
        let res = table.add(RelationDescriptor::new(a, b, RelationKind::Alias, EpochId(2)), EpochId(2));
        assert!(res.is_err());
    }
}
