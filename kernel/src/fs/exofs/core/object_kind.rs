// kernel/src/fs/exofs/core/object_kind.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ObjectKind — type sémantique, matrice de permissions, contraintes de classe
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLES :
//   KIND-01 : PathIndex est TOUJOURS Class2, jamais Class1.
//   KIND-02 : Secret → BlobId JAMAIS exposé (règle SEC-07).
//   KIND-03 : Code → validation ELF obligatoire avant exec.
//   KIND-04 : Config → validation de schéma obligatoire avant rechargement.
//   KIND-05 : Relation → toujours Class1 (immuable une fois créée).
//   ONDISK-01 : #[repr(u8)] pour layout disque déterministe.

use crate::fs::exofs::core::error::ExofsError;
use crate::fs::exofs::core::object_class::ObjectClass;

// ─────────────────────────────────────────────────────────────────────────────
// ObjectKind
// ─────────────────────────────────────────────────────────────────────────────

/// Type sémantique d'un objet logique ExoFS.
///
/// Stocké sur 1 octet dans l'ObjectHeader on-disk.
/// Détermine les règles de validation, permissions et visibilité du BlobId.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ObjectKind {
    /// Données génériques (fichier binaire ou texte).
    Blob = 0,
    /// Code exécutable — validation ELF obligatoire avant exec (règle KIND-03).
    Code = 1,
    /// Configuration structurée — validation schéma obligatoire (règle KIND-04).
    Config = 2,
    /// Secret chiffré — BlobId JAMAIS exposé (règle KIND-02 / SEC-07).
    Secret = 3,
    /// Répertoire (PathIndex) — toujours Class2, mutation atomique CoW.
    PathIndex = 4,
    /// Lien typé entre deux objets — toujours Class1 (règle KIND-05).
    Relation = 5,
}

impl ObjectKind {
    // ────────────────────────────────────────────────────────────────────────────
    // Classification
    // ────────────────────────────────────────────────────────────────────────────

    /// Construction depuis octet on-disk. None si variante inconnue.
    #[inline]
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Blob),
            1 => Some(Self::Code),
            2 => Some(Self::Config),
            3 => Some(Self::Secret),
            4 => Some(Self::PathIndex),
            5 => Some(Self::Relation),
            _ => None,
        }
    }

    /// Sérialisation on-disk.
    #[inline]
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Retourne vrai si cet objet est un répertoire (PathIndex).
    #[inline]
    pub fn is_directory(self) -> bool {
        matches!(self, Self::PathIndex)
    }

    /// Retourne vrai si le BlobId ne doit JAMAIS être exposé (règle SEC-07).
    #[inline]
    pub fn is_secret(self) -> bool {
        matches!(self, Self::Secret)
    }

    /// Retourne vrai si l'objet exige une validation avant exécution.
    #[inline]
    pub fn requires_exec_validation(self) -> bool {
        matches!(self, Self::Code)
    }

    /// Retourne vrai si l'objet exige une validation de schéma avant rechargement.
    #[inline]
    pub fn requires_schema_validation(self) -> bool {
        matches!(self, Self::Config)
    }

    /// Retourne vrai si l'objet est une relation (immuable).
    #[inline]
    pub fn is_relation(self) -> bool {
        matches!(self, Self::Relation)
    }

    /// Retourne vrai si ce kind accepte les données inline.
    ///
    /// PathIndex et Relation n'ont pas de données inline utilisateur.
    #[inline]
    pub fn supports_inline_data(self) -> bool {
        matches!(self, Self::Blob | Self::Config | Self::Code | Self::Secret)
    }

    // ────────────────────────────────────────────────────────────────────────────
    // Classe par défaut
    // ────────────────────────────────────────────────────────────────────────────

    /// Classe par défaut pour ce kind.
    ///
    /// PathIndex → Class2 (règle KIND-01).
    /// Relation  → Class1 (règle KIND-05).
    /// Secret    → Class1 (règle CLASS-04).
    /// Autres    → Class1 (content-addressed par défaut).
    #[inline]
    pub fn default_class(self) -> ObjectClass {
        match self {
            Self::PathIndex => ObjectClass::Class2,
            _ => ObjectClass::Class1,
        }
    }

    /// Vrai si la classe proposée est valide pour ce kind.
    pub fn class_allowed(self, class: ObjectClass) -> bool {
        match self {
            Self::PathIndex => class == ObjectClass::Class2,
            _ => true, // Tous les autres kinds acceptent les deux classes.
        }
    }

    // ────────────────────────────────────────────────────────────────────────────
    // Matrice de permissions par opération
    // ────────────────────────────────────────────────────────────────────────────

    /// Vérifie si cette opération est sémantiquement valide pour ce kind.
    ///
    /// Exemples :
    /// - Exec sur un non-Code → WrongObjectKind
    /// - List sur un non-PathIndex → WrongObjectKind
    pub fn check_kind_operation(self, op: KindOperation) -> Result<(), ExofsError> {
        let ok = match (self, op) {
            // Exec uniquement sur Code.
            (Self::Code, KindOperation::Exec) => true,
            (_, KindOperation::Exec) => false,
            // List uniquement sur PathIndex.
            (Self::PathIndex, KindOperation::List) => true,
            (_, KindOperation::List) => false,
            // AddChild / RemoveChild uniquement sur PathIndex.
            (Self::PathIndex, KindOperation::AddChild) => true,
            (Self::PathIndex, KindOperation::RemoveChild) => true,
            (_, KindOperation::AddChild) => false,
            (_, KindOperation::RemoveChild) => false,
            // Snapshot compatible avec tous les kinds sauf Relation.
            (Self::Relation, KindOperation::Snapshot) => false,
            (_, KindOperation::Snapshot) => true,
            // Lecture/écriture générique autorisée sur tous les kinds.
            _ => true,
        };
        if ok {
            Ok(())
        } else {
            Err(ExofsError::WrongObjectKind)
        }
    }

    // ────────────────────────────────────────────────────────────────────────────
    // Propriétés de stockage
    // ────────────────────────────────────────────────────────────════════════════

    /// Vrai si la déduplication est applicable par défaut pour ce kind.
    ///
    /// Les Secrets ne sont jamais dédupliqués (le blob chiffré inclut un nonce).
    #[inline]
    pub fn dedup_applicable(self) -> bool {
        !matches!(self, Self::Secret | Self::Relation)
    }

    /// Vrai si la compression est applicable pour ce kind.
    ///
    /// Code et Config bénéficient généralement de la compression.
    /// Secrets : déjà chiffrés (AES-GCM) → entropie élevée, pas de gain.
    #[inline]
    pub fn compression_applicable(self) -> bool {
        !matches!(self, Self::Secret)
    }

    /// Vrai si ce kind doit être inclus dans les snapshots permanents.
    #[inline]
    pub fn included_in_snapshot(self) -> bool {
        true // Tous les kinds sont snapshotables.
    }

    /// Préfixe de chemin logique utilisé dans les journaux de diagnostic.
    pub fn log_prefix(self) -> &'static str {
        match self {
            Self::Blob => "blob",
            Self::Code => "code",
            Self::Config => "config",
            Self::Secret => "secret",
            Self::PathIndex => "dir",
            Self::Relation => "rel",
        }
    }
}

impl core::fmt::Display for ObjectKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Blob => write!(f, "Blob"),
            Self::Code => write!(f, "Code"),
            Self::Config => write!(f, "Config"),
            Self::Secret => write!(f, "Secret"),
            Self::PathIndex => write!(f, "PathIndex"),
            Self::Relation => write!(f, "Relation"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// KindOperation — opérations dont la validité dépend du kind
// ─────────────────────────────────────────────────────────────────────────────

/// Opérations dont la validité sémantique dépend du kind d'objet.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum KindOperation {
    /// Exécution (appel système exec, uniquement Code).
    Exec,
    /// Liste des entrées (uniquement PathIndex).
    List,
    /// Ajout d'une entrée enfant (uniquement PathIndex).
    AddChild,
    /// Suppression d'une entrée enfant (uniquement PathIndex).
    RemoveChild,
    /// Création d'un snapshot de l'objet.
    Snapshot,
    /// Lecture du contenu brut.
    Read,
    /// Écriture du contenu.
    Write,
    /// Suppression de l'objet.
    Delete,
    /// Lecture des métadonnées.
    Stat,
}

// ─────────────────────────────────────────────────────────────────────────────
// Utilitaires de validation on-disk
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie qu'un octet on-disk correspond à un ObjectKind valide.
///
/// Appelé lors du parsing d'ObjectHeader pour détecter la corruption.
#[inline]
pub fn validate_kind_byte(v: u8) -> Result<ObjectKind, ExofsError> {
    ObjectKind::from_u8(v).ok_or(ExofsError::CorruptedStructure)
}

/// Vérifie la cohérence kind + class lors de la désérialisation on-disk.
///
/// Retourne ExofsError::CorruptedStructure si combinaison invalide.
pub fn validate_kind_class(kind: ObjectKind, class: ObjectClass) -> Result<(), ExofsError> {
    if !kind.class_allowed(class) {
        return Err(ExofsError::CorruptedStructure);
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// KindConstraints — limites structurelles par kind
// ─────────────────────────────────────────────────────────────────────────────

/// Contraintes structurelles d'un ObjectKind.
///
/// Ces données sont statiques et déterminées à la compilation.
/// Consultées lors de la validation des opérations et de l'allocation.
#[derive(Copy, Clone, Debug)]
pub struct KindConstraints {
    /// Taille maximale du contenu brut (octets).
    pub max_content_bytes: u64,
    /// Taille minimale du contenu brut (octets). 0 = vide autorisé.
    pub min_content_bytes: u64,
    /// Nombre maximal d'extents par objet.
    pub max_extents: u32,
    /// Niveau de stockage préféré (0 = hot, 1 = warm, 2 = cold).
    pub preferred_tier: u8,
    /// Durée de rétention minimale en epochs (0 = pas de rétention).
    pub min_retention_epochs: u32,
    /// Le kind peut être inline dans l'ObjectHeader.
    pub inline_allowed: bool,
    /// Le kind requiert une validation de structure lors du read.
    pub requires_struct_check: bool,
    /// Overhead CoW estimé (pourcentage × 10, ex. 15 = 1.5%).
    pub cow_overhead_pct10: u16,
}

impl KindConstraints {
    /// Retourne les contraintes pour un kind donné.
    pub const fn for_kind(kind: ObjectKind) -> &'static KindConstraints {
        match kind {
            ObjectKind::Blob => &KIND_CONSTRAINTS_BLOB,
            ObjectKind::Code => &KIND_CONSTRAINTS_CODE,
            ObjectKind::Config => &KIND_CONSTRAINTS_CONFIG,
            ObjectKind::Secret => &KIND_CONSTRAINTS_SECRET,
            ObjectKind::PathIndex => &KIND_CONSTRAINTS_PATHINDEX,
            ObjectKind::Relation => &KIND_CONSTRAINTS_RELATION,
        }
    }
}

/// Contraintes pour Blob : contenu arbitraire, très grande taille admise.
static KIND_CONSTRAINTS_BLOB: KindConstraints = KindConstraints {
    max_content_bytes: 256 * 1024 * 1024 * 1024, // 256 Gio
    min_content_bytes: 0,
    max_extents: 65536,
    preferred_tier: 1, // warm
    min_retention_epochs: 0,
    inline_allowed: true,
    requires_struct_check: false,
    cow_overhead_pct10: 15, // 1.5%
};

/// Contraintes pour Code : ELF ou bytecode, taille limitée.
static KIND_CONSTRAINTS_CODE: KindConstraints = KindConstraints {
    max_content_bytes: 256 * 1024 * 1024, // 256 Mio
    min_content_bytes: 64,                // header ELF minimal
    max_extents: 4096,
    preferred_tier: 0,       // hot (fréquemment exécuté)
    min_retention_epochs: 2, // conservé au moins 2 époques
    inline_allowed: false,
    requires_struct_check: true, // validation ELF
    cow_overhead_pct10: 25,      // 2.5%
};

/// Contraintes pour Config : petits fichiers structurés.
static KIND_CONSTRAINTS_CONFIG: KindConstraints = KindConstraints {
    max_content_bytes: 4 * 1024 * 1024, // 4 Mio
    min_content_bytes: 0,
    max_extents: 64,
    preferred_tier: 0, // hot
    min_retention_epochs: 1,
    inline_allowed: true,
    requires_struct_check: true, // validation schéma
    cow_overhead_pct10: 10,
};

/// Contraintes pour Secret : chiffré, taille limitée, isolation maximale.
static KIND_CONSTRAINTS_SECRET: KindConstraints = KindConstraints {
    max_content_bytes: 64 * 1024, // 64 Kio
    min_content_bytes: 1,
    max_extents: 4,
    preferred_tier: 2,       // cold (rotation lente)
    min_retention_epochs: 4, // conservé longtemps
    inline_allowed: false,   // jamais inline (SEC-03)
    requires_struct_check: false,
    cow_overhead_pct10: 50, // 5% (padding cryptographique)
};

/// Contraintes pour PathIndex : grand arbre mutable, CoW fréquent.
static KIND_CONSTRAINTS_PATHINDEX: KindConstraints = KindConstraints {
    max_content_bytes: 64 * 1024 * 1024, // 64 Mio
    min_content_bytes: 0,
    max_extents: 8192,
    preferred_tier: 0, // hot (accès fréquent)
    min_retention_epochs: 0,
    inline_allowed: true,
    requires_struct_check: false,
    cow_overhead_pct10: 30, // 3% (CoW fréquent)
};

/// Contraintes pour Relation : triplette fixe, très petit.
static KIND_CONSTRAINTS_RELATION: KindConstraints = KindConstraints {
    max_content_bytes: 4096, // 4 Kio — a triplet
    min_content_bytes: 64,
    max_extents: 2,
    preferred_tier: 1, // warm
    min_retention_epochs: 1,
    inline_allowed: true,
    requires_struct_check: false,
    cow_overhead_pct10: 5,
};

// ─────────────────────────────────────────────────────────────────────────────
// KindRetentionPolicy — politique de rétention par kind
// ─────────────────────────────────────────────────────────────────────────────

/// Politique de rétention d'un objet après suppression logique.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum KindRetentionPolicy {
    /// Suppression immédiate au GC (aucune rétention post-delete).
    Immediate,
    /// Rétention d'un epoch complet avant GC autorisé.
    OneEpoch,
    /// Rétention de deux epochs avant GC (volatile + stable).
    TwoEpochs,
    /// Rétention configurée — consultez min_retention_epochs.
    Configured,
}

impl ObjectKind {
    /// Retourne la politique de rétention de ce kind.
    pub fn retention_policy(self) -> KindRetentionPolicy {
        match self {
            ObjectKind::Blob => KindRetentionPolicy::OneEpoch,
            ObjectKind::Code => KindRetentionPolicy::TwoEpochs,
            ObjectKind::Config => KindRetentionPolicy::TwoEpochs,
            ObjectKind::Secret => KindRetentionPolicy::TwoEpochs,
            ObjectKind::PathIndex => KindRetentionPolicy::OneEpoch,
            ObjectKind::Relation => KindRetentionPolicy::OneEpoch,
        }
    }

    /// Retourne la taille maximale de contenu brut admise pour ce kind.
    pub fn max_content_bytes(self) -> u64 {
        KindConstraints::for_kind(self).max_content_bytes
    }

    /// Retourne le niveau de stockage préféré (tier).
    ///
    /// 0 = hot (SSD NVMe), 1 = warm (SSD SATA), 2 = cold (HDD).
    pub fn preferred_storage_tier(self) -> u8 {
        KindConstraints::for_kind(self).preferred_tier
    }

    /// Vrai si ce kind requiert une vérification de structure supplémentaire.
    ///
    /// Code → vérification du magic ELF.
    /// Config → vérification du schéma de configuration.
    pub fn requires_struct_check(self) -> bool {
        KindConstraints::for_kind(self).requires_struct_check
    }

    /// Overhead CoW estimé en pourcent × 10 (ex. 15 = 1.5%).
    ///
    /// Utilisé par l'estimateur de capacité lors des commits d'epoch.
    pub fn cow_overhead_pct10(self) -> u16 {
        KindConstraints::for_kind(self).cow_overhead_pct10
    }

    /// Vrai si une mise à jour vers un kind cible est compatible.
    ///
    /// Transitions autorisées :
    ///   - Blob    → Config (si le contenu est validé comme schéma).
    ///   - Config  → Blob   (dégradation vers binaire arbitraire).
    ///   - Autres  → immutables (le kind ne change jamais).
    pub fn can_upgrade_to(self, target: Self) -> bool {
        matches!(
            (self, target),
            (ObjectKind::Blob, ObjectKind::Config) | (ObjectKind::Config, ObjectKind::Blob)
        )
    }

    /// Mappage POSIX st_mode type (bits 0170000).
    ///
    /// Utilisé lors de l'export POSIX d'un filesystem ExoFS.
    pub fn posix_type_bits(self) -> u32 {
        match self {
            ObjectKind::Blob => 0o100000,      // regular file
            ObjectKind::Code => 0o100000,      // regular file (exécutable)
            ObjectKind::Config => 0o100000,    // regular file
            ObjectKind::Secret => 0o100000,    // regular file
            ObjectKind::PathIndex => 0o040000, // directory
            ObjectKind::Relation => 0o120000,  // symlink (sémantique proche)
        }
    }

    /// Description textuelle du kind (pour interface utilisateur).
    pub fn description(self) -> &'static str {
        match self {
            ObjectKind::Blob => "Binary large object",
            ObjectKind::Code => "Executable code (ELF/bytecode)",
            ObjectKind::Config => "Structured configuration data",
            ObjectKind::Secret => "Encrypted secret material",
            ObjectKind::PathIndex => "Path index (directory)",
            ObjectKind::Relation => "Typed relation (edge)",
        }
    }

    /// Préfixe MIME pour ce kind (approximatif, pour l'export).
    pub fn mime_type_hint(self) -> &'static str {
        match self {
            ObjectKind::Blob => "application/octet-stream",
            ObjectKind::Code => "application/x-elf",
            ObjectKind::Config => "application/json",
            ObjectKind::Secret => "application/octet-stream",
            ObjectKind::PathIndex => "inode/directory",
            ObjectKind::Relation => "application/x-exofs-relation",
        }
    }

    /// Vrai si ce kind est compatible avec l'export vers le VFS POSIX.
    pub fn posix_exportable(self) -> bool {
        !matches!(self, ObjectKind::Secret | ObjectKind::Relation)
    }

    /// Vrai si des données inline (InlineData) peuvent être stockées directement
    /// dans l'ObjectHeader pour ce kind (optimisation lecture).
    pub fn allows_inline_data(self) -> bool {
        KindConstraints::for_kind(self).inline_allowed
    }

    /// Vrai si ce kind peut avoir des relations entrantes.
    pub fn accepts_incoming_relations(self) -> bool {
        !matches!(self, ObjectKind::Relation)
    }

    /// Vérifie que la taille de contenu respecte les bornes du kind.
    pub fn validate_content_size(self, size_bytes: u64) -> Result<(), ExofsError> {
        let c = KindConstraints::for_kind(self);
        if size_bytes < c.min_content_bytes {
            return Err(ExofsError::InvalidArgument);
        }
        if size_bytes > c.max_content_bytes {
            return Err(ExofsError::ObjectTooLarge);
        }
        Ok(())
    }

    /// Vérifie qu'un nombre d'extents respecte le maximum du kind.
    pub fn validate_extent_count(self, count: u32) -> Result<(), ExofsError> {
        if count > KindConstraints::for_kind(self).max_extents {
            return Err(ExofsError::ObjectTooLarge);
        }
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// KindOperation — implémentation supplémentaire
// ─────────────────────────────────────────────────────────────────────────────

impl KindOperation {
    /// Vrai si cette opération nécessite les droits EXEC.
    pub fn requires_exec_right(self) -> bool {
        matches!(self, Self::Exec)
    }

    /// Vrai si cette opération modifie la structure du namespace.
    pub fn modifies_namespace(self) -> bool {
        matches!(self, Self::AddChild | Self::RemoveChild)
    }

    /// Vrai si cette opération est en lecture seule.
    pub fn is_read_only(self) -> bool {
        matches!(self, Self::Read | Self::Stat | Self::List | Self::Exec)
    }

    /// Description textuelle de l'opération.
    pub fn name(self) -> &'static str {
        match self {
            Self::Exec => "exec",
            Self::List => "list",
            Self::AddChild => "add_child",
            Self::RemoveChild => "remove_child",
            Self::Snapshot => "snapshot",
            Self::Read => "read",
            Self::Write => "write",
            Self::Delete => "delete",
            Self::Stat => "stat",
        }
    }

    /// Vrai si l'opération crée une dépendance entre deux objets.
    pub fn creates_dependency(self) -> bool {
        matches!(self, Self::AddChild | Self::Snapshot)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Matrice de droits minimaux requis par (kind, operation)
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne les droits minimaux requis pour une opération sur un kind.
///
/// Utilisé par le layer de vérification de sécurité pour éviter
/// les erreurs d'embarquement de rights incorrects.
pub fn minimum_rights_for_kind_op(kind: ObjectKind, op: KindOperation) -> u32 {
    use crate::fs::exofs::core::rights::{
        RIGHT_CREATE, RIGHT_DELETE, RIGHT_EXEC, RIGHT_LIST, RIGHT_READ, RIGHT_SNAPSHOT_CREATE,
        RIGHT_STAT, RIGHT_WRITE,
    };
    match (kind, op) {
        (_, KindOperation::Stat) => RIGHT_STAT,
        (_, KindOperation::Read) => RIGHT_READ,
        (_, KindOperation::Write) => RIGHT_WRITE,
        (_, KindOperation::Delete) => RIGHT_DELETE,
        (_, KindOperation::Snapshot) => RIGHT_SNAPSHOT_CREATE,
        (ObjectKind::Code, KindOperation::Exec) => RIGHT_EXEC,
        (ObjectKind::PathIndex, KindOperation::List) => RIGHT_LIST,
        (ObjectKind::PathIndex, KindOperation::AddChild) => RIGHT_WRITE | RIGHT_CREATE,
        (ObjectKind::PathIndex, KindOperation::RemoveChild) => RIGHT_WRITE | RIGHT_DELETE,
        _ => RIGHT_READ,
    }
}

/// Vérifie qu'une opération est cohérente avec le kind et les droits fournis.
///
/// Point d'entrée centralisé combinant la matrice kind × op et les droits.
pub fn check_kind_op_rights(
    kind: ObjectKind,
    op: KindOperation,
    rights: u32,
) -> Result<(), ExofsError> {
    // 1. Vérification kind × op (sémantique).
    kind.check_kind_operation(op)?;
    // 2. Vérification des droits minimaux.
    let required = minimum_rights_for_kind_op(kind, op);
    if rights & required != required {
        return Err(ExofsError::PermissionDenied);
    }
    Ok(())
}
