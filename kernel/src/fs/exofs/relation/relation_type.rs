//! RelationType — types et méta-données d'une relation ExoFS (no_std).

/// Nature d'une relation entre deux blobs / inodes.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RelationKind {
    Parent       = 0x01,   // from est parent de to.
    Child        = 0x02,   // from est enfant de to.
    Symlink      = 0x03,   // from est un lien symbolique vers to.
    HardLink     = 0x04,   // from est un hard link vers to.
    Refcount     = 0x05,   // Référence de comptage (P-Blob).
    Snapshot     = 0x06,   // from est une capture de to.
    SnapshotBase = 0x07,   // to est la base de from.
    Dedup        = 0x08,   // from et to partagent des chunks.
    Clone        = 0x09,   // from est un clone CoW de to.
    CrossRef     = 0x0A,   // Référence croisée arbitraire.
}

impl RelationKind {
    pub fn is_hierarchical(self) -> bool {
        matches!(self, Self::Parent | Self::Child)
    }

    pub fn is_link(self) -> bool {
        matches!(self, Self::Symlink | Self::HardLink)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Parent       => "parent",
            Self::Child        => "child",
            Self::Symlink      => "symlink",
            Self::HardLink     => "hardlink",
            Self::Refcount     => "refcount",
            Self::Snapshot     => "snapshot",
            Self::SnapshotBase => "snapshot_base",
            Self::Dedup        => "dedup",
            Self::Clone        => "clone",
            Self::CrossRef     => "crossref",
        }
    }
}

/// Poids / priorité d'une relation (pour le parcours de graphe).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RelationWeight(pub u32);

impl RelationWeight {
    pub const DEFAULT: Self = Self(1);
    pub const STRONG:  Self = Self(10);
}

/// Type complet d'une relation avec son poids.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelationType {
    pub kind:   RelationKind,
    pub weight: RelationWeight,
}

impl RelationType {
    pub fn new(kind: RelationKind) -> Self {
        Self { kind, weight: RelationWeight::DEFAULT }
    }

    pub fn with_weight(kind: RelationKind, weight: u32) -> Self {
        Self { kind, weight: RelationWeight(weight) }
    }
}
