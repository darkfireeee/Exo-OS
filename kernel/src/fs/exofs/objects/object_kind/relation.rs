// kernel/src/fs/exofs/objects/object_kind/relation.rs
//
// Objets Relation — arête typée entre deux objets du graphe ExoFS.

use crate::fs::exofs::core::ObjectId;

/// Type d'une relation entre objets.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum RelationKind {
    /// Relation parent→enfant (structure hiérarchique).
    Parent    = 0,
    /// Relation de dépendance (A dépend de B).
    DependsOn = 1,
    /// Alias (A est un alias de B).
    Alias     = 2,
    /// Lien symbolique (A pointe vers B par chemin).
    Symlink   = 3,
}

/// Descripteur d'une relation entre deux objets.
#[derive(Copy, Clone, Debug)]
pub struct RelationDescriptor {
    pub src:  ObjectId,
    pub dst:  ObjectId,
    pub kind: RelationKind,
}
