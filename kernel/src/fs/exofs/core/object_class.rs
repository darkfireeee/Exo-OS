// kernel/src/fs/exofs/core/object_class.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ObjectClass — Classe 1 (immuable) vs Classe 2 (CoW mutable)
// Transitions, invariants, politique CoW, contexte CoW, matrice de classe
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLES COMPLÈTES :
//   CLASS-01 : Class1 → JAMAIS modifié. ObjectId = Blake3(blob_id || owner_cap).
//              L'ObjectId est calculé UNE SEULE FOIS à la création.
//   CLASS-02 : Class2 → mutation CoW UNIQUEMENT. ObjectId = compteur monotone.
//              Le blob précédent reste valide jusqu'au GC de l'epoch précédent.
//   CLASS-03 : PathIndex est TOUJOURS Class2. Toute mutation exige un CoW atomique.
//   CLASS-04 : Secret est TOUJOURS Class1. L'immuabilité du contenu chiffré
//              est une propriété de sécurité non-négociable.
//   CLASS-05 : Une promotion Class1→Class2 crée un NOUVEL ObjectId Class2.
//              L'ObjectId Class1 d'origine est mis à jour dans le PathIndex.
//   CLASS-06 : BlobId exposé uniquement pour non-Secrets (règle SEC-07).
//   CLASS-07 : Relation est TOUJOURS Class1 (immuable une fois créée).
//   CLASS-08 : Tout CoW génère obligatoirement un nouveau BlobId AVANT compression
//              et AVANT chiffrement (règle HASH-01 prévalente).
//   CLASS-09 : L'epoch_id d'un objet Class2 est mis à jour à chaque CoW commit.
//   CLASS-10 : Un objet Class2 peut revenir à Class1 via "freeze" (snapshot).

use core::fmt;
use crate::fs::exofs::core::object_kind::ObjectKind;
use crate::fs::exofs::core::types::{BlobId, EpochId, Extent};
use crate::fs::exofs::core::error::ExofsError;

// ─────────────────────────────────────────────────────────────────────────────
// ObjectClass
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// ObjectClass
// ─────────────────────────────────────────────────────────────────────────────

/// Classe d'un objet logique ExoFS.
///
/// Class1 : immuable, content-addressed, ObjectId dérivé du contenu.
/// Class2 : mutable via Copy-on-Write, ObjectId stable à vie.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ObjectClass {
    /// Objet immuable content-addressed (ObjectId = Blake3(blob_id||cap)).
    /// - Jamais modifiable après création.
    /// - BlobId stable et unique pour ce contenu exact.
    /// - Réutilisé par déduplication si BlobId identique détecté.
    Class1 = 1,
    /// Objet mutable via Copy-on-Write (ObjectId = compteur monotone stable).
    /// - Chaque write produit un nouveau BlobId (le blob précédent reste valide
    ///   jusqu'au GC epoch).
    /// - L'ObjectId reste constant à travers toutes les mutations.
    Class2 = 2,
}

impl ObjectClass {
    /// Retourne vrai si l'objet est immuable (Classe 1).
    #[inline]
    pub fn is_immutable(self) -> bool { matches!(self, Self::Class1) }

    /// Retourne vrai si l'objet supporte la mutation CoW (Classe 2).
    #[inline]
    pub fn is_mutable(self) -> bool { matches!(self, Self::Class2) }

    /// Convertit depuis octet on-disk. Retourne None si valeur inconnue.
    #[inline]
    pub fn from_u8(v: u8) -> Option<Self> {
        match v { 1 => Some(Self::Class1), 2 => Some(Self::Class2), _ => None }
    }

    /// Représentation u8 pour écriture on-disk (règle ONDISK-01).
    #[inline]
    pub fn as_u8(self) -> u8 { self as u8 }

    /// Classe par défaut pour un ObjectKind donné.
    ///
    /// | Kind      | Classe par défaut | Raison                       |
    /// |-----------|-------------------|------------------------------|
    /// | PathIndex | Class2            | Règle CLASS-03               |
    /// | Secret    | Class1            | Règle CLASS-04               |
    /// | Relation  | Class1            | Règle CLASS-07               |
    /// | Autres    | Class1            | Content-addressed par défaut |
    #[inline]
    pub fn default_for_kind(kind: ObjectKind) -> Self {
        match kind {
            ObjectKind::PathIndex => Self::Class2,
            _                    => Self::Class1,
        }
    }

    /// Vrai si la classe est fixée pour ce kind (ne peut jamais changer).
    ///
    /// PathIndex → toujours Class2.
    /// Secret, Relation → toujours Class1.
    pub fn is_forced_for_kind(kind: ObjectKind) -> bool {
        matches!(kind, ObjectKind::PathIndex | ObjectKind::Secret | ObjectKind::Relation)
    }

    /// Vrai si la BlobId peut être exposée dans cette classe + kind.
    ///
    /// Règle CLASS-06 + SEC-07 : BlobId jamais exposé pour les Secrets.
    #[inline]
    pub fn blob_id_visible(self, kind: ObjectKind) -> bool {
        !kind.is_secret()
    }

    /// Vérifie si cette classe est compatible avec l'opération demandée.
    pub fn check_operation(self, op: ClassOperation) -> Result<(), ExofsError> {
        match (self, op) {
            (Self::Class1, ClassOperation::Write)      => Err(ExofsError::WrongObjectClass),
            (Self::Class1, ClassOperation::Truncate)   => Err(ExofsError::WrongObjectClass),
            (Self::Class1, ClassOperation::Extend)     => Err(ExofsError::WrongObjectClass),
            (Self::Class1, ClassOperation::SetMeta)    => Err(ExofsError::WrongObjectClass),
            (Self::Class1, ClassOperation::Freeze)     => Err(ExofsError::WrongObjectClass),
            _ => Ok(()),
        }
    }

    /// Retourne toutes les opérations autorisées pour cette classe.
    pub fn allowed_operations(self) -> &'static [ClassOperation] {
        match self {
            Self::Class1 => &[
                ClassOperation::Read,
                ClassOperation::Snapshot,
                ClassOperation::Delete,
                ClassOperation::StatMetadata,
                ClassOperation::InspectBlobId,
            ],
            Self::Class2 => &[
                ClassOperation::Read,
                ClassOperation::Write,
                ClassOperation::Truncate,
                ClassOperation::Extend,
                ClassOperation::SetMeta,
                ClassOperation::Snapshot,
                ClassOperation::Delete,
                ClassOperation::StatMetadata,
                ClassOperation::InspectBlobId,
                ClassOperation::Freeze,
            ],
        }
    }

    /// Retourne le nom textuel de la classe (pour logs kernel).
    pub fn name(self) -> &'static str {
        match self { Self::Class1 => "Class1", Self::Class2 => "Class2" }
    }

    /// Vrai si une transition de cette classe vers `target` est possible.
    pub fn can_transition_to(self, target: Self) -> bool { self != target }

    /// Valide la cohérence d'une transition, en tenant compte du kind.
    pub fn validate_transition(
        self,
        target: Self,
        kind: ObjectKind,
    ) -> Result<ClassTransition, ExofsError> {
        if self == target { return Err(ExofsError::InvalidArgument); }
        if target == Self::Class1 && kind == ObjectKind::PathIndex {
            return Err(ExofsError::WrongObjectClass);
        }
        if target == Self::Class2 && kind == ObjectKind::Secret {
            return Err(ExofsError::WrongObjectClass);
        }
        if kind == ObjectKind::Relation {
            return Err(ExofsError::WrongObjectClass);
        }
        Ok(ClassTransition { from: self, to: target, reason: TransitionReason::Explicit })
    }
}

impl fmt::Display for ObjectClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Class1 => write!(f, "Class1(immutable)"),
            Self::Class2 => write!(f, "Class2(CoW)"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ClassOperation — opérations soumises à contrôle de classe
// ─────────────────────────────────────────────────────────────────────────────

/// Opérations soumises à vérification de classe.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ClassOperation {
    /// Lecture du contenu (toujours permise).
    Read,
    /// Écriture partielle ou totale (requiert Class2).
    Write,
    /// Troncature du contenu (requiert Class2).
    Truncate,
    /// Extension du contenu, append (requiert Class2).
    Extend,
    /// Mise à jour des métadonnées (requiert Class2).
    SetMeta,
    /// Création d'un snapshot (autorisée sur les deux classes).
    Snapshot,
    /// Suppression logique (soft-delete).
    Delete,
    /// Lecture des métadonnées (stat, kind, epoch…).
    StatMetadata,
    /// Exposition du BlobId (interdit pour les Secrets — CLASS-06).
    InspectBlobId,
    /// Gel d'un objet Class2 → Class1 (freeze / snapshot permanent).
    Freeze,
}

impl ClassOperation {
    /// Vrai si cette opération nécessite Class2.
    #[inline]
    pub fn requires_class2(self) -> bool {
        matches!(self, Self::Write | Self::Truncate | Self::Extend
                     | Self::SetMeta | Self::Freeze)
    }

    /// Vrai si cette opération est en lecture seule.
    #[inline]
    pub fn is_read_only(self) -> bool {
        matches!(self, Self::Read | Self::StatMetadata | Self::InspectBlobId)
    }

    /// Description courte pour les journaux kernel.
    pub fn name(self) -> &'static str {
        match self {
            Self::Read          => "read",
            Self::Write         => "write",
            Self::Truncate      => "truncate",
            Self::Extend        => "extend",
            Self::SetMeta       => "setmeta",
            Self::Snapshot      => "snapshot",
            Self::Delete        => "delete",
            Self::StatMetadata  => "stat",
            Self::InspectBlobId => "inspect_blobid",
            Self::Freeze        => "freeze",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ClassTransition — enregistrement d'une transition de classe approuvée
// ─────────────────────────────────────────────────────────────────────────────

/// Transition de classe validée, retournée par `validate_transition`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ClassTransition {
    pub from:   ObjectClass,
    pub to:     ObjectClass,
    pub reason: TransitionReason,
}

/// Raison d'une transition de classe.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TransitionReason {
    /// Promotion explicite demandée par le processus.
    Explicit,
    /// Promotion automatique par le moteur CoW (première écriture).
    CowAutoPromote,
    /// Freeze : Class2 → Class1 lors d'un snapshot.
    SnapshotFreeze,
    /// Recovery : classe recalculée depuis on-disk.
    Recovery,
}

impl fmt::Display for ClassTransition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} → {} ({})",
            self.from.name(), self.to.name(),
            match self.reason {
                TransitionReason::Explicit       => "explicit",
                TransitionReason::CowAutoPromote => "cow-auto",
                TransitionReason::SnapshotFreeze => "freeze",
                TransitionReason::Recovery       => "recovery",
            }
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CowPolicy — politique de Copy-on-Write pour les objets Class2
// ─────────────────────────────────────────────────────────────────────────────

/// Politique CoW appliquée lors d'une mutation d'objet Class2.
///
/// Garanties invariantes de tout CoW (règle CLASS-08) :
///   1. Nouveau BlobId = Blake3(nouvelles données brutes) AVANT compression.
///   2. L'ancien BlobId reste valide jusqu'au GC de l'epoch précédent.
///   3. La mise à jour est atomique au niveau de l'epoch commit.
///   4. ObjectId Class2 ne change JAMAIS (règle CLASS-02).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CowPolicy {
    /// CoW complet : écriture d'un nouveau P-Blob intégral.
    /// Utilisé quand > 50% du blob est modifié.
    FullCopy,
    /// CoW partiel par Extent : seul le ou les extents modifiés sont copiés.
    /// Réduit l'amplification d'écriture pour les modifications localisées.
    PartialExtent,
    /// CoW inline : les données tiennent dans la structure d'objet elle-même.
    /// Pas de nouveau P-Blob alloué.
    Inline,
    /// CoW différé (lazy CoW) : copie planifiée mais pas encore exécutée.
    /// Utilisé lors d'un snapshot pour différer la copie.
    Deferred,
}

impl CowPolicy {
    /// Description lisible de la politique.
    pub fn name(self) -> &'static str {
        match self {
            Self::FullCopy      => "full-copy",
            Self::PartialExtent => "partial-extent",
            Self::Inline        => "inline",
            Self::Deferred      => "deferred",
        }
    }

    /// Vrai si un nouveau P-Blob doit être alloué avec cette politique.
    pub fn needs_new_blob(self) -> bool {
        matches!(self, Self::FullCopy | Self::PartialExtent)
    }

    /// Vrai si cette politique est compatible avec la déduplication.
    pub fn dedup_eligible(self) -> bool {
        matches!(self, Self::FullCopy | Self::PartialExtent)
    }
}

/// Seuil inline-data pour la sélection de politique CoW.
const COW_INLINE_MAX: u64 = crate::fs::exofs::core::constants::INLINE_DATA_MAX as u64;
/// Seuil partiel : si delta/total < 20%, on fait un CoW partiel.
const COW_PARTIAL_PCT: u64 = 20;

/// Détermine la politique CoW optimale depuis les métriques de mutation.
///
/// Algorithme :
///   total ≤ INLINE_DATA_MAX          → Inline
///   (delta * 100 / total) < 20%      → PartialExtent
///   sinon                            → FullCopy
pub fn choose_cow_policy(total_bytes: u64, modified_bytes: u64) -> CowPolicy {
    if total_bytes <= COW_INLINE_MAX {
        return CowPolicy::Inline;
    }
    if total_bytes > 0 {
        let pct = modified_bytes.saturating_mul(100) / total_bytes;
        if pct < COW_PARTIAL_PCT {
            return CowPolicy::PartialExtent;
        }
    }
    CowPolicy::FullCopy
}

/// Determine si un CoW différé est préférable.
///
/// CoW différé sélectionné quand un snapshot vient d'être créé
/// et qu'aucune écriture n'a encore eu lieu dans ce nouvel epoch.
pub fn should_defer_cow(snapshotted_in_current_epoch: bool, write_pending: bool) -> bool {
    snapshotted_in_current_epoch && !write_pending
}

// ─────────────────────────────────────────────────────────────────────────────
// CowContext — suivi d'une opération CoW en cours
// ─────────────────────────────────────────────────────────────────────────────

/// Contexte d'une opération Copy-on-Write en cours.
///
/// Stocké temporairement dans l'ObjectHeader pendant la durée de l'écriture.
/// Effacé après commit ou rollback de l'epoch.
#[derive(Clone, Debug)]
pub struct CowContext {
    /// BlobId de l'objet avant la mutation.
    pub old_blob_id:     BlobId,
    /// BlobId de l'objet après la mutation.
    pub new_blob_id:     Option<BlobId>,
    /// Epoch de début de cette opération CoW.
    pub started_epoch:   EpochId,
    /// Politique CoW sélectionnée.
    pub policy:          CowPolicy,
    /// Extent modifié (Some si PartialExtent, None si FullCopy/Inline).
    pub modified_extent: Option<Extent>,
    /// Taille totale de l'objet avant mutation.
    pub total_bytes:     u64,
    /// Nombre d'octets modifiés.
    pub modified_bytes:  u64,
    /// Le CoW a-t-il été committé ?
    pub committed:       bool,
    /// Le CoW a-t-il été annulé ?
    pub aborted:         bool,
}

impl CowContext {
    /// Crée un nouveau contexte CoW et sélectionne la politique optimale.
    pub fn new(
        old_blob_id:     BlobId,
        started_epoch:   EpochId,
        total_bytes:     u64,
        modified_bytes:  u64,
        modified_extent: Option<Extent>,
    ) -> Self {
        let policy = choose_cow_policy(total_bytes, modified_bytes);
        Self {
            old_blob_id,
            new_blob_id: None,
            started_epoch,
            policy,
            modified_extent,
            total_bytes,
            modified_bytes,
            committed: false,
            aborted:   false,
        }
    }

    /// Finalise le CoW avec le nouveau BlobId calculé.
    ///
    /// RÈGLE HASH-01 : `new_blob_id` DOIT avoir été calculé sur les données
    /// brutes non-compressées avant l'appel à cette fonction.
    pub fn commit(&mut self, new_blob_id: BlobId) -> Result<(), ExofsError> {
        if self.committed || self.aborted {
            return Err(ExofsError::InvalidArgument);
        }
        if new_blob_id.is_zero() {
            return Err(ExofsError::InvalidArgument);
        }
        // Nouveau blob doit être différent de l'ancien (sinon CoW inutile).
        if new_blob_id.ct_eq(&self.old_blob_id) {
            return Err(ExofsError::InvalidArgument);
        }
        self.new_blob_id = Some(new_blob_id);
        self.committed   = true;
        Ok(())
    }

    /// Annule le CoW : l'ancien BlobId reste actif.
    pub fn rollback(&mut self) {
        self.new_blob_id = None;
        self.aborted     = true;
    }

    /// BlobId actif (nouveau si committé, ancien sinon).
    pub fn active_blob_id(&self) -> &BlobId {
        if self.committed {
            self.new_blob_id.as_ref().unwrap_or(&self.old_blob_id)
        } else {
            &self.old_blob_id
        }
    }

    /// Vrai si ce contexte est en attente de commit ou rollback.
    #[inline]
    pub fn is_pending(&self) -> bool { !self.committed && !self.aborted }

    /// Vrai si le CoW modifie assez pour justifier une dédup.
    pub fn dedup_eligible(&self) -> bool {
        self.policy.dedup_eligible() && self.modified_bytes > 0
    }

    /// Amplification d'écriture × 100 (évite les f64, règle ARITH-01).
    ///
    /// FullCopy → total/modified × 100.
    /// PartialExtent → 100 (≈1×).
    /// Inline / Deferred → 0.
    pub fn write_amplification_x100(&self) -> u64 {
        if self.modified_bytes == 0 { return 0; }
        match self.policy {
            CowPolicy::FullCopy      =>
                self.total_bytes.saturating_mul(100) / self.modified_bytes,
            CowPolicy::PartialExtent => 100,
            CowPolicy::Inline | CowPolicy::Deferred => 0,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ClassConstraints — ensemble des contraintes structurelles d'un objet
// ─────────────────────────────────────────────────────────────────────────────

/// Contraintes de classe applicables à un objet (kind + class combinés).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ClassConstraints {
    /// La classe est fixée et ne peut jamais changer pour ce kind.
    pub class_fixed:       bool,
    /// La promotion Class1 → Class2 est autorisée.
    pub promotion_allowed: bool,
    /// Le gel (Class2 → Class1) est autorisé.
    pub freeze_allowed:    bool,
    /// Les mutations CoW sont autorisées.
    pub cow_allowed:       bool,
    /// Le BlobId peut être exposé (non-Secret).
    pub blob_id_exposable: bool,
    /// La déduplication peut être appliquée.
    pub dedup_allowed:     bool,
    /// La compression peut être appliquée.
    pub compress_allowed:  bool,
}

impl ClassConstraints {
    /// Construit les contraintes pour une combinaison kind + class.
    pub fn for_kind_class(kind: ObjectKind, class: ObjectClass) -> Self {
        Self {
            class_fixed:       ObjectClass::is_forced_for_kind(kind),
            promotion_allowed: can_promote_to_class2(class, kind),
            freeze_allowed:    class == ObjectClass::Class2
                                   && kind != ObjectKind::PathIndex,
            cow_allowed:       class == ObjectClass::Class2,
            blob_id_exposable: !kind.is_secret(),
            dedup_allowed:     kind.dedup_applicable(),
            compress_allowed:  kind.compression_applicable(),
        }
    }

    /// Vrai si toutes les mutations sont refusées.
    pub fn is_fully_restricted(self) -> bool {
        !self.cow_allowed && !self.promotion_allowed && !self.freeze_allowed
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Fonctions publiques de validation et de décision
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie les invariants de classe pour un objet donné (désérialisation).
///
/// Retourne `ExofsError::CorruptedStructure` si un invariant est violé.
pub fn validate_class_invariants(
    class: ObjectClass,
    kind:  ObjectKind,
) -> Result<(), ExofsError> {
    // CLASS-03 : PathIndex doit être Class2.
    if kind == ObjectKind::PathIndex && class != ObjectClass::Class2 {
        return Err(ExofsError::CorruptedStructure);
    }
    // CLASS-04 : Secret doit être Class1.
    if kind == ObjectKind::Secret && class != ObjectClass::Class1 {
        return Err(ExofsError::CorruptedStructure);
    }
    // CLASS-07 : Relation doit être Class1.
    if kind == ObjectKind::Relation && class != ObjectClass::Class1 {
        return Err(ExofsError::CorruptedStructure);
    }
    Ok(())
}

/// Vrai si la combinaison class + kind peut subir une promotion Class1 → Class2.
///
/// Conditions :
///   1. Objet actuellement Class1.
///   2. Kind ≠ PathIndex (déjà Class2).
///   3. Kind ≠ Secret (immuabilité sécurité).
///   4. Kind ≠ Relation (immuabilité sémantique).
pub fn can_promote_to_class2(class: ObjectClass, kind: ObjectKind) -> bool {
    class == ObjectClass::Class1
        && !matches!(kind, ObjectKind::PathIndex | ObjectKind::Secret | ObjectKind::Relation)
}

/// Vrai si l'objet doit rester Class1 indépendamment des demandes de mutation.
pub fn must_remain_class1(kind: ObjectKind) -> bool {
    matches!(kind, ObjectKind::Secret | ObjectKind::Relation)
}

/// Classe qu'aura un objet après création d'un snapshot (toujours Class1).
pub fn class_after_snapshot(_current: ObjectClass) -> ObjectClass {
    ObjectClass::Class1
}

/// Vrai si deux objets de classes différentes peuvent partager un BlobId.
///
/// - Class1 + Class1 → TOUJOURS (déduplication).
/// - Class1 + Class2 → TEMPORAIRE admis (pending CoW).
/// - Class2 + Class2 → JAMAIS (isolation CoW).
pub fn can_share_blob(class_a: ObjectClass, class_b: ObjectClass) -> bool {
    !(class_a == ObjectClass::Class2 && class_b == ObjectClass::Class2)
}

/// Point d'entrée unique pour la vérification class + kind + opération.
///
/// Centralise toute la logique de matrice pour éviter les incohérences.
pub fn class_operation_check(
    class: ObjectClass,
    kind:  ObjectKind,
    op:    ClassOperation,
) -> Result<(), ExofsError> {
    // 1. Invariants structurels class + kind.
    validate_class_invariants(class, kind)?;
    // 2. Opération vs classe.
    class.check_operation(op)?;
    // 3. ContraClasses combinées kind + op.
    match (kind, op) {
        // CLASS-06 / SEC-07 : BlobId d'un Secret jamais exposé.
        (ObjectKind::Secret, ClassOperation::InspectBlobId) =>
            Err(ExofsError::SecretBlobIdLeakPrevented),
        // CLASS-03 : PathIndex ne peut pas être gelé.
        (ObjectKind::PathIndex, ClassOperation::Freeze) =>
            Err(ExofsError::WrongObjectClass),
        _ => Ok(()),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Sérialisation / désérialisation de ClassTransition (journal de recovery)
// ─────────────────────────────────────────────────────────────────────────────

/// Sérialise une ClassTransition sur 3 octets pour le journal on-disk.
///
/// Format : [from_u8 | to_u8 | reason_u8]
pub fn serialize_transition(t: &ClassTransition) -> [u8; 3] {
    [
        t.from.as_u8(),
        t.to.as_u8(),
        match t.reason {
            TransitionReason::Explicit       => 0,
            TransitionReason::CowAutoPromote => 1,
            TransitionReason::SnapshotFreeze => 2,
            TransitionReason::Recovery       => 3,
        },
    ]
}

/// Désérialise une ClassTransition depuis 3 octets on-disk.
///
/// Retourne `ExofsError::CorruptedStructure` si les octets sont invalides.
pub fn deserialize_transition(b: [u8; 3]) -> Result<ClassTransition, ExofsError> {
    let from   = ObjectClass::from_u8(b[0]).ok_or(ExofsError::CorruptedStructure)?;
    let to     = ObjectClass::from_u8(b[1]).ok_or(ExofsError::CorruptedStructure)?;
    let reason = match b[2] {
        0 => TransitionReason::Explicit,
        1 => TransitionReason::CowAutoPromote,
        2 => TransitionReason::SnapshotFreeze,
        3 => TransitionReason::Recovery,
        _ => return Err(ExofsError::CorruptedStructure),
    };
    Ok(ClassTransition { from, to, reason })
}
