// kernel/src/fs/exofs/core/rights.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Droits ExoFS — RightsMask, constantes, ensembles pré-définis, vérification
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Les droits ExoFS sont stockés dans les CapTokens comme bitmask u32.
// Bits 0-9   : droits VFS génériques (définis dans security/)
// Bits 10-15 : droits ExoFS-spécifiques (définis ici)
// Bits 16-31 : réservés pour extensions
//
// RÈGLE SEC-03 : chaque opération priviligiée doit vérifier le Right associé.
// RÈGLE SEC-07 : BlobId d'un Secret → JAMAIS exposé, même avec INSPECT_CONTENT.

// ─────────────────────────────────────────────────────────────────────────────
// Constantes de droits
// ─────────────────────────────────────────────────────────────────────────────

/// Droit de lire le contenu d'un objet (données brutes ou inline).
pub const RIGHT_READ: u32 = 1 << 0;
/// Droit d'écrire / modifier le contenu (Class2 uniquement).
pub const RIGHT_WRITE: u32 = 1 << 1;
/// Droit de créer de nouveaux objets dans le namespace.
pub const RIGHT_CREATE: u32 = 1 << 2;
/// Droit de supprimer un objet (soft-delete).
pub const RIGHT_DELETE: u32 = 1 << 3;
/// Droit de lire les métadonnées (stat, kind, class, epoch).
pub const RIGHT_STAT: u32 = 1 << 4;
/// Droit de modifier les métadonnées (rename, chmod équivalent).
pub const RIGHT_SETMETA: u32 = 1 << 5;
/// Droit de lister le contenu d'un PathIndex.
pub const RIGHT_LIST: u32 = 1 << 6;
/// Droit d'exécuter un objet de type Code (après validation ELF).
pub const RIGHT_EXEC: u32 = 1 << 7;
/// Droit de changer le propriétaire d'un objet.
pub const RIGHT_CHOWN: u32 = 1 << 8;
/// Droit de changer les droits d'accès d'un objet.
pub const RIGHT_CHMOD: u32 = 1 << 9;
/// Droit d'inspecter le contenu hash (expose BlobId, sauf Secret — règle SEC-07).
pub const RIGHT_INSPECT_CONTENT: u32 = 1 << 10;
/// Droit de créer un snapshot permanent de l'epoch courant.
pub const RIGHT_SNAPSHOT_CREATE: u32 = 1 << 11;
/// Droit de créer une relation typée entre objets.
pub const RIGHT_RELATION_CREATE: u32 = 1 << 12;
/// Droit de déclencher le GC manuellement.
pub const RIGHT_GC_TRIGGER: u32 = 1 << 13;
/// Droit d'exporter des objets via ExoAR/stream.
pub const RIGHT_EXPORT: u32 = 1 << 14;
/// Droit d'importer des blobs depuis une archive externe.
pub const RIGHT_IMPORT: u32 = 1 << 15;
/// Droit d'administration — accès privilégié complet (format, reconfiguration).
pub const RIGHT_ADMIN: u32 = 1 << 16;

/// Masque de tous les droits ExoFS définis.
pub const ALL_RIGHTS: u32 = 0x0000_FFFF;
/// Masque des droits restreints (nécessitent élévation de privilèges).
pub const PRIVILEGED_RIGHTS: u32 = RIGHT_GC_TRIGGER | RIGHT_IMPORT | RIGHT_SNAPSHOT_CREATE;

// ─────────────────────────────────────────────────────────────────────────────
// RightsMask — wrapper typé autour du bitmask u32
// ─────────────────────────────────────────────────────────────────────────────

/// Bitmask de droits ExoFS encapsulé dans un type newtype.
///
/// Garantit que seuls des droits valides sont manipulés.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct RightsMask(pub u32);

impl RightsMask {
    /// Masque vide — aucun droit.
    pub const NONE: Self = Self(0);
    /// Tous les droits ExoFS.
    pub const ALL: Self = Self(ALL_RIGHTS);
    /// Droits en lecture seule.
    pub const READ_ONLY: Self = Self(RIGHT_READ | RIGHT_STAT | RIGHT_LIST);
    /// Droits lecture + exécution (fichiers exécutables).
    pub const READ_EXEC: Self = Self(RIGHT_READ | RIGHT_STAT | RIGHT_EXEC);
    /// Droits lecture-écriture complets (sans gestion de droits).
    pub const READ_WRITE: Self = Self(
        RIGHT_READ
            | RIGHT_WRITE
            | RIGHT_CREATE
            | RIGHT_DELETE
            | RIGHT_STAT
            | RIGHT_SETMETA
            | RIGHT_LIST,
    );
    /// Droits administrateur ExoFS (tous les droits).
    pub const ADMIN: Self = Self(ALL_RIGHTS);

    /// Crée un masque depuis des bits bruts.
    #[inline]
    pub fn from_bits(bits: u32) -> Self {
        // Masque uniquement les bits définis pour éviter les droits fantômes.
        Self(bits & ALL_RIGHTS)
    }

    /// Retourne les bits bruts pour stockage on-disk (dans CapToken).
    #[inline]
    pub fn bits(self) -> u32 {
        self.0
    }

    /// Vrai si ce masque contient le droit `right`.
    #[inline]
    pub fn has(self, right: u32) -> bool {
        self.0 & right == right
    }

    /// Vrai si ce masque contient TOUS les droits de `other`.
    #[inline]
    pub fn contains_all(self, other: RightsMask) -> bool {
        self.0 & other.0 == other.0
    }

    /// Vrai si ce masque contient AU MOINS UN droit de `other`.
    #[inline]
    pub fn contains_any(self, other: RightsMask) -> bool {
        self.0 & other.0 != 0
    }

    /// Union de deux masques.
    #[inline]
    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Intersection de deux masques.
    #[inline]
    pub fn intersect(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// Soustraction : retire les droits de `other` de ce masque.
    #[inline]
    pub fn remove(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// Ajoute un droit au masque.
    #[inline]
    pub fn add(&mut self, right: u32) {
        self.0 |= right & ALL_RIGHTS;
    }

    /// Retire un droit du masque.
    #[inline]
    pub fn revoke(&mut self, right: u32) {
        self.0 &= !right;
    }

    /// Vrai si le masque est vide (aucun droit).
    #[inline]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Nombre de droits actifs dans ce masque.
    #[inline]
    pub fn count(self) -> u32 {
        self.0.count_ones()
    }

    /// Vrai si aucun droit privilégié n'est inclus.
    #[inline]
    pub fn is_unprivileged(self) -> bool {
        self.0 & PRIVILEGED_RIGHTS == 0
    }

    /// Réduit la granularité au minimum pour ce kind (principe du moindre privilège).
    ///
    /// Les Secrets n'accordent jamais INSPECT_CONTENT ni EXPORT.
    pub fn restrict_for_secret(self) -> Self {
        Self(self.0 & !(RIGHT_INSPECT_CONTENT | RIGHT_EXPORT))
    }
}

impl core::ops::BitOr for RightsMask {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

impl core::ops::BitAnd for RightsMask {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        self.intersect(rhs)
    }
}

impl core::ops::BitOrAssign for RightsMask {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl core::fmt::Display for RightsMask {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Rights(0x{:04x})", self.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Fonctions de vérification (compatibilité ascendante avec l'ancienne API)
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie si un bitmask de droits inclut INSPECT_CONTENT.
#[inline]
pub fn has_inspect_content(bits: u32) -> bool {
    bits & RIGHT_INSPECT_CONTENT != 0
}
/// Vérifie si un bitmask inclut SNAPSHOT_CREATE.
#[inline]
pub fn has_snapshot_create(bits: u32) -> bool {
    bits & RIGHT_SNAPSHOT_CREATE != 0
}
/// Vérifie si un bitmask inclut RELATION_CREATE.
#[inline]
pub fn has_relation_create(bits: u32) -> bool {
    bits & RIGHT_RELATION_CREATE != 0
}
/// Vérifie si un bitmask inclut GC_TRIGGER.
#[inline]
pub fn has_gc_trigger(bits: u32) -> bool {
    bits & RIGHT_GC_TRIGGER != 0
}
/// Vérifie si un bitmask inclut EXPORT.
#[inline]
pub fn has_export(bits: u32) -> bool {
    bits & RIGHT_EXPORT != 0
}
/// Vérifie si un bitmask inclut IMPORT.
#[inline]
pub fn has_import(bits: u32) -> bool {
    bits & RIGHT_IMPORT != 0
}
/// Vérifie si un bitmask inclut WRITE.
#[inline]
pub fn has_write(bits: u32) -> bool {
    bits & RIGHT_WRITE != 0
}
/// Vérifie si un bitmask inclut READ.
#[inline]
pub fn has_read(bits: u32) -> bool {
    bits & RIGHT_READ != 0
}
/// Vérifie si un bitmask inclut EXEC.
#[inline]
pub fn has_exec(bits: u32) -> bool {
    bits & RIGHT_EXEC != 0
}
/// Vérifie si un bitmask inclut CREATE.
#[inline]
pub fn has_create(bits: u32) -> bool {
    bits & RIGHT_CREATE != 0
}
/// Vérifie si un bitmask inclut DELETE.
#[inline]
pub fn has_delete(bits: u32) -> bool {
    bits & RIGHT_DELETE != 0
}

/// Vrai si la combinaison de droits expose un Secret (interdit — règle SEC-07).
///
/// Aucune combinaison de droits ne doit exposer le BlobId d'un Secret.
/// Cette fonction est appelée dans le layer syscall avant toute exposition.
#[inline]
pub fn would_expose_secret(bits: u32) -> bool {
    // INSPECT_CONTENT sur un Secret = violation SEC-07
    // La vérification du kind doit être faite par l'appelant
    bits & RIGHT_INSPECT_CONTENT != 0
}

// ─────────────────────────────────────────────────────────────────────────────
// RightsError — erreurs spécifiques à la gestion des droits
// ─────────────────────────────────────────────────────────────────────────────

/// Erreurs spécifiques à l'application des règles de droits.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RightsError {
    /// Droit requis absent du masque fourni.
    MissingRight(u32),
    /// Tentative de délégation d'un droit non possédé.
    DelegationExceeds,
    /// Tentative d'élévation de droits refusée.
    EscalationDenied,
    /// Droits insuffisants pour cette opération sur ce kind.
    InsufficientForKindOp,
    /// Droit privilégié utilisé sans les droits root.
    PrivilegedRightDenied,
    /// Le masque de droits contient des bits réservés.
    UnknownRightBits,
}

impl core::fmt::Display for RightsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::MissingRight(r) => write!(f, "missing right bitfield 0x{:X}", r),
            Self::DelegationExceeds => write!(f, "delegation exceeds grantor rights"),
            Self::EscalationDenied => write!(f, "rights escalation denied"),
            Self::InsufficientForKindOp => write!(f, "insufficient rights for kind+op"),
            Self::PrivilegedRightDenied => write!(f, "privileged right denied"),
            Self::UnknownRightBits => write!(f, "unknown right bits in mask"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RightsPolicy — politique de contrôle d'accès
// ─────────────────────────────────────────────────────────────────────────────

/// Politique d'application des droits.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RightsPolicy {
    /// Vérification stricte : tout droit manquant = erreur.
    Strict,
    /// Mode audit : les droits manquants sont loggés mais pas bloquants.
    Audit,
    /// Mode permissif : uniquement les droits critiques sont vérifiés.
    Permissive,
}

// ─────────────────────────────────────────────────────────────────────────────
// RightsValidator — validateur de droits centralisé
// ─────────────────────────────────────────────────────────────────────────────

/// Validateur centralisé de droits.
///
/// Encapsule la politique et l'historique de vérification.
/// Toutes les décisions de droits passent idéalement par ce validateur
/// pour garantir la traçabilité.
#[derive(Clone, Debug)]
pub struct RightsValidator {
    /// Droits en possession de l'appelant.
    pub granted: RightsMask,
    /// Politique de contrôle.
    pub policy: RightsPolicy,
    /// Nombre de vérifications réussies depuis création.
    pub ok_count: u32,
    /// Nombre de vérifications échouées depuis création.
    pub ko_count: u32,
}

impl RightsValidator {
    /// Crée un nouveau validateur.
    pub fn new(granted: RightsMask, policy: RightsPolicy) -> Self {
        Self {
            granted,
            policy,
            ok_count: 0,
            ko_count: 0,
        }
    }

    /// Vérifie qu'un droit est présent dans le masque accordé.
    pub fn check(&mut self, required: u32) -> Result<(), RightsError> {
        if self.granted.has(required) {
            self.ok_count = self.ok_count.saturating_add(1);
            Ok(())
        } else {
            self.ko_count = self.ko_count.saturating_add(1);
            match self.policy {
                RightsPolicy::Strict | RightsPolicy::Audit => {
                    Err(RightsError::MissingRight(required))
                }
                RightsPolicy::Permissive => {
                    // En mode permissif, seules les erreurs critiques bloquent.
                    if required & PRIVILEGED_RIGHTS != 0 {
                        Err(RightsError::PrivilegedRightDenied)
                    } else {
                        Ok(())
                    }
                }
            }
        }
    }

    /// Vérifie un ensemble de droits (tous requis).
    pub fn check_all(&mut self, required_mask: u32) -> Result<(), RightsError> {
        if self.granted.contains_all(RightsMask(required_mask)) {
            self.ok_count = self.ok_count.saturating_add(1);
            Ok(())
        } else {
            self.ko_count = self.ko_count.saturating_add(1);
            let missing = required_mask & !self.granted.bits();
            Err(RightsError::MissingRight(missing))
        }
    }

    /// Vrai si au moins un des droits du masque est présent.
    pub fn check_any(&mut self, mask: u32) -> bool {
        self.granted.contains_any(RightsMask(mask))
    }

    /// Vérifie qu'aucun bit inconnu n'est présent dans un masque externe.
    pub fn validate_external_mask(bits: u32) -> Result<(), RightsError> {
        if bits & !ALL_RIGHTS != 0 {
            return Err(RightsError::UnknownRightBits);
        }
        Ok(())
    }

    /// Taux d'échec × 100 (évite les f64, règle ARITH-01).
    pub fn failure_rate_x100(&self) -> u32 {
        let total = self.ok_count.saturating_add(self.ko_count);
        if total == 0 {
            return 0;
        }
        self.ko_count.saturating_mul(100) / total
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RightsDelegation — règles de délégation de droits
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'une tentative de délégation de droits.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RightsDelegation {
    /// Droits du délégant (source).
    pub grantor_rights: RightsMask,
    /// Droits demandés pour le délégué.
    pub requested: RightsMask,
    /// Droits effectivement accordés au délégué.
    pub delegated: RightsMask,
    /// Vrai si la délégation est complète (requested ⊆ delegated).
    pub full: bool,
}

/// Calcule les droits délégables depuis un grantor vers un grantee.
///
/// RÈGLE : un délégant ne peut jamais accorder plus que ce qu'il possède.
/// Le résultat est l'intersection de (requested, grantor_rights).
pub fn delegate_rights(grantor: RightsMask, requested: RightsMask) -> RightsDelegation {
    // Droits effectivement délégables = intersection.
    let delegated = grantor.intersect(requested);
    let full = delegated == requested;
    RightsDelegation {
        grantor_rights: grantor,
        requested,
        delegated,
        full,
    }
}

/// Vérifie si une délégation est valide (pas d'élévation).
///
/// Retourne une erreur si le demandeur tente d'obtenir plus que le grantor.
pub fn check_delegation(
    grantor: RightsMask,
    requested: RightsMask,
) -> Result<RightsMask, RightsError> {
    if !grantor.contains_all(requested) {
        return Err(RightsError::DelegationExceeds);
    }
    Ok(requested)
}

// ─────────────────────────────────────────────────────────────────────────────
// Héritage de droits (parent → enfant dans un PathIndex)
// ─────────────────────────────────────────────────────────────────────────────

/// Calcule les droits hérités par un objet enfant depuis son parent PathIndex.
///
/// L'héritage applique une restriction automatique :
///   - Les droits privilégiés (GC_TRIGGER, IMPORT) ne s'héritent pas.
///   - Les droits administratifs (CHOWN, CHMOD) sont conservés.
///   - L'intersection avec `allowed_mask` est appliquée.
pub fn inherit_rights(parent_rights: RightsMask, allowed_mask: RightsMask) -> RightsMask {
    // Retrait des droits privilégiés non-hérités.
    let filtered = parent_rights.remove(RightsMask(PRIVILEGED_RIGHTS));
    filtered.intersect(allowed_mask)
}

/// Droits minimaux pour accéder en lecture à un objet d'un kind donné.
///
/// Secret → READ seul ne suffit pas, INSPECT_CONTENT requis.
pub fn minimum_read_rights(kind: crate::fs::exofs::core::object_kind::ObjectKind) -> RightsMask {
    use crate::fs::exofs::core::object_kind::ObjectKind;
    match kind {
        ObjectKind::Secret => RightsMask(RIGHT_READ | RIGHT_INSPECT_CONTENT),
        ObjectKind::Code => RightsMask(RIGHT_READ | RIGHT_EXEC),
        ObjectKind::PathIndex => RightsMask(RIGHT_LIST),
        _ => RightsMask(RIGHT_READ),
    }
}

/// Droits minimaux pour écrire dans un objet d'un kind donné.
pub fn minimum_write_rights(kind: crate::fs::exofs::core::object_kind::ObjectKind) -> RightsMask {
    use crate::fs::exofs::core::object_kind::ObjectKind;
    match kind {
        ObjectKind::PathIndex => RightsMask(RIGHT_WRITE | RIGHT_CREATE),
        ObjectKind::Secret => RightsMask(RIGHT_WRITE | RIGHT_SETMETA),
        _ => RightsMask(RIGHT_WRITE),
    }
}

/// Droits minimaux pour supprimer un objet d'un kind donné.
pub fn minimum_delete_rights(kind: crate::fs::exofs::core::object_kind::ObjectKind) -> RightsMask {
    use crate::fs::exofs::core::object_kind::ObjectKind;
    match kind {
        ObjectKind::Secret => RightsMask(RIGHT_DELETE | RIGHT_SETMETA),
        ObjectKind::PathIndex => RightsMask(RIGHT_DELETE | RIGHT_WRITE),
        _ => RightsMask(RIGHT_DELETE),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RightsAuditEntry — entrée de log d'audit pour la sécurité
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée d'audit pour une décision de droit.
///
/// Enregistrée dans le journal de sécurité du kernel pour analyse forensique.
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct RightsAuditEntry {
    /// Epoch où la décision a eu lieu.
    pub epoch_id: u64,
    /// Rights demandés.
    pub requested: u32,
    /// Rights accordés.
    pub granted: u32,
    /// Action approuvée (1) ou refusée (0).
    pub allowed: u8,
    /// Kind de l'objet concerné.
    pub object_kind: u8,
    /// Réservé pour alignement.
    pub _reserved: [u8; 2],
}

impl RightsAuditEntry {
    /// Crée une entrée d'audit approuvée.
    pub fn approved(epoch: u64, requested: u32, granted: u32, kind: u8) -> Self {
        Self {
            epoch_id: epoch,
            requested,
            granted,
            allowed: 1,
            object_kind: kind,
            _reserved: [0; 2],
        }
    }
    /// Crée une entrée d'audit refusée.
    pub fn denied(epoch: u64, requested: u32, granted: u32, kind: u8) -> Self {
        Self {
            epoch_id: epoch,
            requested,
            granted,
            allowed: 0,
            object_kind: kind,
            _reserved: [0; 2],
        }
    }
    /// Vrai si la décision était un refus.
    pub fn is_denied(self) -> bool {
        self.allowed == 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Vérification complète droits + kind + operation
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie les droits pour une opération kind × op avec le masque fourni.
///
/// Point d'entrée centralisé pour le layer syscall.
/// Combine la matrice kind × op minimum_rights avec le masque accordé.
pub fn check_rights_for_kind_op(
    mask: RightsMask,
    kind: crate::fs::exofs::core::object_kind::ObjectKind,
    op: crate::fs::exofs::core::object_kind::KindOperation,
) -> Result<(), RightsError> {
    use crate::fs::exofs::core::object_kind::KindOperation;
    let required = match op {
        KindOperation::Read => minimum_read_rights(kind).bits(),
        KindOperation::Write => minimum_write_rights(kind).bits(),
        KindOperation::Delete => minimum_delete_rights(kind).bits(),
        KindOperation::Exec => RIGHT_EXEC,
        KindOperation::List => RIGHT_LIST,
        KindOperation::AddChild => RIGHT_WRITE | RIGHT_CREATE,
        KindOperation::RemoveChild => RIGHT_WRITE | RIGHT_DELETE,
        KindOperation::Snapshot => RIGHT_SNAPSHOT_CREATE,
        KindOperation::Stat => RIGHT_STAT,
    };
    if mask.bits() & required != required {
        return Err(RightsError::MissingRight(required & !mask.bits()));
    }
    Ok(())
}

/// Détecte une tentative d'élévation de droits.
///
/// Retourne une erreur si `new_mask` contient des droits absents de `current`.
pub fn detect_escalation(current: RightsMask, new_mask: RightsMask) -> Result<(), RightsError> {
    if new_mask.bits() & !current.bits() != 0 {
        return Err(RightsError::EscalationDenied);
    }
    Ok(())
}
