//! relation_type.rs — Types et métadonnées des relations ExoFS
//!
//! Règles appliquées :
//!  - ARITH-02 : arithmétique vérifiée sur les poids
//!  - ONDISK-03: aucun AtomicU64 dans les structs repr(C)

// ─────────────────────────────────────────────────────────────────────────────
// RelationKind
// ─────────────────────────────────────────────────────────────────────────────

/// Nature sémantique d'une relation entre deux blobs / objets ExoFS.
///
/// Chaque variant a une valeur on-disk u8 fixe pour la sérialisation.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RelationKind {
    /// `from` est le répertoire parent de `to`.
    Parent = 0x01,
    /// `from` est un enfant de `to`.
    Child = 0x02,
    /// `from` est un lien symbolique vers `to`.
    Symlink = 0x03,
    /// `from` est un hard link vers `to`.
    HardLink = 0x04,
    /// Relation de comptage de références (P-Blob).
    Refcount = 0x05,
    /// `from` est une capture instantanée de `to`.
    Snapshot = 0x06,
    /// `to` est la base de laquelle `from` a été capturé.
    SnapshotBase = 0x07,
    /// `from` et `to` partagent des chunks (déduplication).
    Dedup = 0x08,
    /// `from` est un clone Copy-on-Write de `to`.
    Clone = 0x09,
    /// Référence croisée arbitraire (extensible).
    CrossRef = 0x0A,
    /// Dépendance de données : `to` doit exister pour que `from` soit lisible.
    DataDep = 0x0B,
    /// Métadonnées associées : `from` enrichit les métadonnées de `to`.
    Metadata = 0x0C,
}

impl RelationKind {
    /// Convertit un octet on-disk en `RelationKind`.
    ///
    /// Retourne `None` si la valeur est inconnue.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0x01 => Some(Self::Parent),
            0x02 => Some(Self::Child),
            0x03 => Some(Self::Symlink),
            0x04 => Some(Self::HardLink),
            0x05 => Some(Self::Refcount),
            0x06 => Some(Self::Snapshot),
            0x07 => Some(Self::SnapshotBase),
            0x08 => Some(Self::Dedup),
            0x09 => Some(Self::Clone),
            0x0A => Some(Self::CrossRef),
            0x0B => Some(Self::DataDep),
            0x0C => Some(Self::Metadata),
            _ => None,
        }
    }

    /// Valeur on-disk.
    #[inline]
    pub const fn to_u8(self) -> u8 {
        self as u8
    }

    /// `true` si la relation est hiérarchique (parent/enfant).
    #[inline]
    pub fn is_hierarchical(self) -> bool {
        matches!(self, Self::Parent | Self::Child)
    }

    /// `true` si la relation est un lien (symlink ou hard link).
    #[inline]
    pub fn is_link(self) -> bool {
        matches!(self, Self::Symlink | Self::HardLink)
    }

    /// `true` si la relation implique un partage de contenu (dédup/clone/snapshot).
    #[inline]
    pub fn is_content_sharing(self) -> bool {
        matches!(
            self,
            Self::Dedup | Self::Clone | Self::Snapshot | Self::SnapshotBase
        )
    }

    /// `true` si la relation constitue une dépendance forte (suppression impossible
    /// sans nettoyer la relation).
    pub fn is_strong_dependency(self) -> bool {
        matches!(
            self,
            Self::Parent | Self::Refcount | Self::SnapshotBase | Self::DataDep
        )
    }

    /// Étiquette lisible (pour logs, debug).
    pub fn label(self) -> &'static str {
        match self {
            Self::Parent => "parent",
            Self::Child => "child",
            Self::Symlink => "symlink",
            Self::HardLink => "hardlink",
            Self::Refcount => "refcount",
            Self::Snapshot => "snapshot",
            Self::SnapshotBase => "snapshot_base",
            Self::Dedup => "dedup",
            Self::Clone => "clone",
            Self::CrossRef => "crossref",
            Self::DataDep => "data_dep",
            Self::Metadata => "metadata",
        }
    }

    /// `true` si la relation est orientée (from→to a un sens asymétrique).
    pub fn is_directed(self) -> bool {
        !matches!(self, Self::Dedup | Self::CrossRef)
    }

    /// Retourne le kind inverse naturel s'il en existe un.
    pub fn inverse(self) -> Option<Self> {
        match self {
            Self::Parent => Some(Self::Child),
            Self::Child => Some(Self::Parent),
            Self::Snapshot => Some(Self::SnapshotBase),
            Self::SnapshotBase => Some(Self::Snapshot),
            _ => None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationWeight
// ─────────────────────────────────────────────────────────────────────────────

/// Poids / priorité d'une relation pour les algorithmes de graphe.
///
/// Un poids est un entier non-signé ; 0 est réservé (relation neutre).
/// Plus le poids est élevé, plus la relation est "forte".
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RelationWeight(pub u32);

impl RelationWeight {
    /// Poids par défaut (relations ordinaires).
    pub const DEFAULT: Self = Self(1);
    /// Poids d'une relation forte (ex. : référence comptée).
    pub const STRONG: Self = Self(10);
    /// Poids maximal autorisé.
    pub const MAX: Self = Self(u32::MAX);
    /// Poids nul (relation désactivée / marqueur de suppression).
    pub const ZERO: Self = Self(0);

    /// Additionne deux poids avec saturation.
    #[inline]
    pub fn saturating_add(self, rhs: Self) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }

    /// Soustrait deux poids avec saturation (jamais négatif).
    #[inline]
    pub fn saturating_sub(self, rhs: Self) -> Self {
        Self(self.0.saturating_sub(rhs.0))
    }

    /// `true` si ce poids indique une relation forte (≥ STRONG).
    #[inline]
    pub fn is_strong(self) -> bool {
        self.0 >= Self::STRONG.0
    }
}

impl Default for RelationWeight {
    fn default() -> Self {
        Self::DEFAULT
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationFlags
// ─────────────────────────────────────────────────────────────────────────────

/// Drapeaux booléens supplémentaires sur une relation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct RelationFlags(pub u16);

impl RelationFlags {
    /// Relation marquée comme supprimée (soft-delete).
    pub const DELETED: Self = Self(0x0001);
    /// Relation vérifiée (intégrité validée).
    pub const VERIFIED: Self = Self(0x0002);
    /// Relation générée automatiquement (pas par l'utilisateur).
    pub const AUTO: Self = Self(0x0004);
    /// Relation temporaire (peut être purgée sans avertissement).
    pub const TEMPORARY: Self = Self(0x0008);

    pub fn has(self, flag: RelationFlags) -> bool {
        self.0 & flag.0 != 0
    }

    pub fn set(self, flag: RelationFlags) -> Self {
        Self(self.0 | flag.0)
    }

    pub fn clear(self, flag: RelationFlags) -> Self {
        Self(self.0 & !flag.0)
    }

    pub fn is_deleted(self) -> bool {
        self.has(Self::DELETED)
    }
    pub fn is_verified(self) -> bool {
        self.has(Self::VERIFIED)
    }
    pub fn is_temporary(self) -> bool {
        self.has(Self::TEMPORARY)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationType
// ─────────────────────────────────────────────────────────────────────────────

/// Type complet d'une relation : nature + poids + drapeaux.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelationType {
    pub kind: RelationKind,
    pub weight: RelationWeight,
    pub flags: RelationFlags,
}

impl RelationType {
    /// Crée un type avec poids par défaut et sans drapeaux.
    pub const fn new(kind: RelationKind) -> Self {
        Self {
            kind,
            weight: RelationWeight::DEFAULT,
            flags: RelationFlags(0),
        }
    }

    /// Crée un type avec poids explicite.
    pub const fn with_weight(kind: RelationKind, weight: u32) -> Self {
        Self {
            kind,
            weight: RelationWeight(weight),
            flags: RelationFlags(0),
        }
    }

    /// Ajoute un drapeau.
    pub fn with_flag(mut self, flag: RelationFlags) -> Self {
        self.flags = self.flags.set(flag);
        self
    }

    /// `true` si la relation n'est pas supprimée et a un poids non nul.
    pub fn is_active(self) -> bool {
        !self.flags.is_deleted() && self.weight != RelationWeight::ZERO
    }

    /// Poids u32 pour la sérialisation.
    pub fn weight_u32(self) -> u32 {
        self.weight.0
    }

    /// Drapeaux u16 pour la sérialisation.
    pub fn flags_u16(self) -> u16 {
        self.flags.0
    }
}

impl Default for RelationType {
    fn default() -> Self {
        Self::new(RelationKind::CrossRef)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationDirection — orientation d'une requête
// ─────────────────────────────────────────────────────────────────────────────

/// Direction de parcours d'une relation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelationDirection {
    /// Relations sortantes (from → to).
    Outgoing,
    /// Relations entrantes (to ← from).
    Incoming,
    /// Les deux directions.
    Both,
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationFilter — filtre de sélection pour requêtes
// ─────────────────────────────────────────────────────────────────────────────

/// Critères de filtrage pour les requêtes sur les relations.
#[derive(Clone, Copy, Debug, Default)]
pub struct RelationFilter {
    /// Restreindre à un type donné (None = tous).
    pub kind: Option<RelationKind>,
    /// Poids minimum (None = aucun minimum).
    pub min_weight: Option<u32>,
    /// Exclure les relations soft-deleted.
    pub active_only: bool,
    /// Direction de la recherche.
    pub direction: Option<RelationDirection>,
}

impl RelationFilter {
    /// Filtre acceptant toutes les relations actives sortantes.
    pub const fn outgoing_active() -> Self {
        RelationFilter {
            kind: None,
            min_weight: None,
            active_only: true,
            direction: Some(RelationDirection::Outgoing),
        }
    }

    /// Filtre pour un type précis, actif uniquement.
    pub fn by_kind(kind: RelationKind) -> Self {
        RelationFilter {
            kind: Some(kind),
            min_weight: None,
            active_only: true,
            direction: None,
        }
    }

    /// `true` si le `RelationType` passe ce filtre.
    pub fn matches(self, rt: RelationType) -> bool {
        if self.active_only && !rt.is_active() {
            return false;
        }
        if let Some(k) = self.kind {
            if rt.kind != k {
                return false;
            }
        }
        if let Some(min_w) = self.min_weight {
            if rt.weight.0 < min_w {
                return false;
            }
        }
        true
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kind_roundtrip() {
        for v in 0x01u8..=0x0C {
            let k = RelationKind::from_u8(v).unwrap();
            assert_eq!(k.to_u8(), v);
        }
    }

    #[test]
    fn test_kind_unknown() {
        assert!(RelationKind::from_u8(0x00).is_none());
        assert!(RelationKind::from_u8(0xFF).is_none());
    }

    #[test]
    fn test_inverse() {
        assert_eq!(RelationKind::Parent.inverse(), Some(RelationKind::Child));
        assert_eq!(RelationKind::CrossRef.inverse(), None);
    }

    #[test]
    fn test_weight_saturating() {
        let w = RelationWeight(u32::MAX);
        assert_eq!(w.saturating_add(RelationWeight(1)), RelationWeight::MAX);
        assert_eq!(
            RelationWeight(0).saturating_sub(RelationWeight(5)),
            RelationWeight::ZERO
        );
    }

    #[test]
    fn test_flags_operations() {
        let f = RelationFlags::default();
        let f = f.set(RelationFlags::DELETED);
        assert!(f.is_deleted());
        let f = f.clear(RelationFlags::DELETED);
        assert!(!f.is_deleted());
    }

    #[test]
    fn test_type_active() {
        let t = RelationType::new(RelationKind::Parent);
        assert!(t.is_active());
        let t2 = t.with_flag(RelationFlags::DELETED);
        assert!(!t2.is_active());
    }

    #[test]
    fn test_filter_by_kind() {
        let f = RelationFilter::by_kind(RelationKind::Snapshot);
        let t_ok = RelationType::new(RelationKind::Snapshot);
        let t_ko = RelationType::new(RelationKind::Clone);
        assert!(f.matches(t_ok));
        assert!(!f.matches(t_ko));
    }

    #[test]
    fn test_is_hierarchical() {
        assert!(RelationKind::Parent.is_hierarchical());
        assert!(!RelationKind::Dedup.is_hierarchical());
    }

    #[test]
    fn test_label_completeness() {
        let kinds = [
            RelationKind::Parent,
            RelationKind::Child,
            RelationKind::Symlink,
            RelationKind::HardLink,
            RelationKind::Refcount,
            RelationKind::Snapshot,
            RelationKind::SnapshotBase,
            RelationKind::Dedup,
            RelationKind::Clone,
            RelationKind::CrossRef,
            RelationKind::DataDep,
            RelationKind::Metadata,
        ];
        for k in kinds {
            assert!(!k.label().is_empty());
        }
    }

    #[test]
    fn test_strong_dependency() {
        assert!(RelationKind::Refcount.is_strong_dependency());
        assert!(!RelationKind::Symlink.is_strong_dependency());
    }

    #[test]
    fn test_content_sharing() {
        assert!(RelationKind::Dedup.is_content_sharing());
        assert!(RelationKind::Clone.is_content_sharing());
        assert!(!RelationKind::Child.is_content_sharing());
    }

    #[test]
    fn test_directed() {
        assert!(RelationKind::Parent.is_directed());
        assert!(!RelationKind::Dedup.is_directed());
    }

    #[test]
    fn test_filter_active_only() {
        let f = RelationFilter {
            active_only: true,
            ..Default::default()
        };
        let deleted = RelationType::new(RelationKind::Clone).with_flag(RelationFlags::DELETED);
        assert!(!f.matches(deleted));
    }

    #[test]
    fn test_weight_is_strong() {
        assert!(RelationWeight::STRONG.is_strong());
        assert!(!RelationWeight::DEFAULT.is_strong());
    }
}
