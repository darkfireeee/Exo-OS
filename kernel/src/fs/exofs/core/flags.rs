// kernel/src/fs/exofs/core/flags.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Flags — bitfields ObjectFlags, ExtentFlags, EpochFlags, SnapshotFlags,
//         MigrationFlags, MountFlags
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════

use crate::fs::exofs::core::error::ExofsError;
use core::fmt;
use core::ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign, Not};

// ─────────────────────────────────────────────────────────────────────────────
// Macro helper pour bitflags no_std
// ─────────────────────────────────────────────────────────────────────────────

/// Macro interne pour générer les méthodes communes sur les bitfields.
macro_rules! impl_flags_common {
    ($T:ty, $Inner:ty) => {
        impl $T {
            /// Vrai si tous les bits de `flag` sont présents.
            #[inline]
            pub fn contains(self, flag: Self) -> bool {
                self.0 & flag.0 == flag.0
            }
            /// Vrai si au moins un bit de `flag` est présent.
            #[inline]
            pub fn contains_any(self, flag: Self) -> bool {
                self.0 & flag.0 != 0
            }
            /// Active les bits de `flag`.
            #[inline]
            pub fn set(&mut self, flag: Self) {
                self.0 |= flag.0;
            }
            /// Désactive les bits de `flag`.
            #[inline]
            pub fn clear(&mut self, flag: Self) {
                self.0 &= !flag.0;
            }
            /// Bascule les bits de `flag`.
            #[inline]
            pub fn toggle(&mut self, flag: Self) {
                self.0 ^= flag.0;
            }
            /// Vrai si aucun bit n'est positionné.
            #[inline]
            pub fn is_empty(self) -> bool {
                self.0 == 0
            }
            /// Retourne les bits bruts.
            #[inline]
            pub fn bits(self) -> $Inner {
                self.0
            }
            /// Crée depuis bits bruts (pas de validation).
            #[inline]
            pub fn from_bits_unchecked(bits: $Inner) -> Self {
                Self(bits)
            }
        }
    };
}

// ─────────────────────────────────────────────────────────────────────────────
// ObjectFlags — attributs d'un LogicalObject
// ─────────────────────────────────────────────────────────────────────────────

/// Flags d'un LogicalObject (u16, stocké on-disk en plain u16).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct ObjectFlags(pub u16);

impl ObjectFlags {
    /// L'objet est marqué pour suppression différée par le GC.
    pub const DELETED: Self = Self(1 << 0);
    /// L'objet contient des données inline (< 512 B) — pas de blob externe.
    pub const INLINE_DATA: Self = Self(1 << 1);
    /// L'objet est chiffré (Secret).
    pub const ENCRYPTED: Self = Self(1 << 2);
    /// L'objet est compressé.
    pub const COMPRESSED: Self = Self(1 << 3);
    /// L'objet est dédupliqué (P-Blob partagé).
    pub const DEDUPED: Self = Self(1 << 4);
    /// L'objet est un snapshot permanent (epoch pinné).
    pub const SNAPSHOT: Self = Self(1 << 5);
    /// L'objet nécessite une synchronisation avant utilisation.
    pub const NEEDS_SYNC: Self = Self(1 << 6);
    /// Écriture CoW en cours (objet Class1 → Class2 en promotion).
    pub const COW_PENDING: Self = Self(1 << 7);
    /// L'objet est en cours de migration vers un tier de stockage différent.
    pub const MIGRATING: Self = Self(1 << 8);
    /// L'objet est verrouillé (accès exclusif par une transaction).
    pub const LOCKED: Self = Self(1 << 9);
    /// L'objet est immuable (jamais modifiable après création).
    pub const IMMUTABLE: Self = Self(1 << 10);
    /// L'objet appartient à un bundle (groupe d'objets liés).
    pub const BUNDLED: Self = Self(1 << 11);
    /// Les métadonnées sont en cache dirty.
    pub const META_DIRTY: Self = Self(1 << 12);
    /// L'objet a été vérifié par le vérificateur d'intégrité en ligne.
    pub const VERIFIED: Self = Self(1 << 13);
    // Bits 14-15 : réservés pour usage futur.

    /// Masque de tous les bits connus.
    pub const ALL_KNOWN: Self = Self(0x3FFF);

    /// Aucun flag positionné.
    pub const NONE: Self = Self(0);

    /// Retourne None si des bits inconnus sont positionnés (règle ONDISK-01).
    pub fn from_bits_validated(bits: u16) -> Result<Self, ExofsError> {
        if bits & !Self::ALL_KNOWN.0 != 0 {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(Self(bits))
    }

    /// Vrai si l'objet peut être lu (pas de dépendance non résolue).
    pub fn is_readable(self) -> bool {
        !self.contains(Self::LOCKED) && !self.contains(Self::DELETED)
    }

    /// Vrai si l'objet peut être écrit.
    pub fn is_writable(self) -> bool {
        !self.contains(Self::LOCKED)
            && !self.contains(Self::DELETED)
            && !self.contains(Self::IMMUTABLE)
            && !self.contains(Self::SNAPSHOT)
    }

    /// Vrai si la combinaison de flags est cohérente (ex: pas INLINE_DATA + DEDUPED).
    pub fn is_valid_combination(self) -> bool {
        // INLINE_DATA incompatible avec DEDUPED (pas de blob externe)
        if self.contains(Self::INLINE_DATA) && self.contains(Self::DEDUPED) {
            return false;
        }
        // IMMUTABLE incompatible avec COW_PENDING
        if self.contains(Self::IMMUTABLE) && self.contains(Self::COW_PENDING) {
            return false;
        }
        // SNAPSHOT incompatible avec DELETED
        if self.contains(Self::SNAPSHOT) && self.contains(Self::DELETED) {
            return false;
        }
        true
    }
}

impl_flags_common!(ObjectFlags, u16);

impl BitOr for ObjectFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitAnd for ObjectFlags {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl BitOrAssign for ObjectFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAndAssign for ObjectFlags {
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0;
    }
}

impl Not for ObjectFlags {
    type Output = Self;
    fn not(self) -> Self {
        Self(!self.0 & Self::ALL_KNOWN.0)
    }
}

impl fmt::Display for ObjectFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        macro_rules! show {
            ($flag:expr, $name:expr) => {
                if self.contains($flag) {
                    if !first {
                        write!(f, "|")?;
                    }
                    first = false;
                    write!(f, $name)?;
                }
            };
        }
        show!(Self::DELETED, "DELETED");
        show!(Self::INLINE_DATA, "INLINE");
        show!(Self::ENCRYPTED, "ENC");
        show!(Self::COMPRESSED, "COMP");
        show!(Self::DEDUPED, "DEDUP");
        show!(Self::SNAPSHOT, "SNAP");
        show!(Self::NEEDS_SYNC, "SYNC");
        show!(Self::COW_PENDING, "COW");
        show!(Self::MIGRATING, "MIGR");
        show!(Self::LOCKED, "LOCK");
        show!(Self::IMMUTABLE, "IMMUT");
        show!(Self::BUNDLED, "BUNDL");
        show!(Self::META_DIRTY, "DIRTY");
        show!(Self::VERIFIED, "VERIF");
        if first {
            write!(f, "NONE")?;
        }
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ObjectFlags builder
// ─────────────────────────────────────────────────────────────────────────────

/// Constructeur fluide pour `ObjectFlags`.
#[derive(Default)]
pub struct ObjectFlagsBuilder(ObjectFlags);

impl ObjectFlagsBuilder {
    pub fn new() -> Self {
        Self(ObjectFlags::NONE)
    }
    pub fn deleted(mut self) -> Self {
        self.0 |= ObjectFlags::DELETED;
        self
    }
    pub fn inline_data(mut self) -> Self {
        self.0 |= ObjectFlags::INLINE_DATA;
        self
    }
    pub fn encrypted(mut self) -> Self {
        self.0 |= ObjectFlags::ENCRYPTED;
        self
    }
    pub fn compressed(mut self) -> Self {
        self.0 |= ObjectFlags::COMPRESSED;
        self
    }
    pub fn deduped(mut self) -> Self {
        self.0 |= ObjectFlags::DEDUPED;
        self
    }
    pub fn snapshot(mut self) -> Self {
        self.0 |= ObjectFlags::SNAPSHOT;
        self
    }
    pub fn immutable(mut self) -> Self {
        self.0 |= ObjectFlags::IMMUTABLE;
        self
    }
    pub fn verified(mut self) -> Self {
        self.0 |= ObjectFlags::VERIFIED;
        self
    }

    /// Construit et valide. Retourne une erreur si la combinaison est invalide.
    pub fn build(self) -> Result<ObjectFlags, ExofsError> {
        if !self.0.is_valid_combination() {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(self.0)
    }

    /// Construit sans validation (debug uniquement).
    pub fn build_unchecked(self) -> ObjectFlags {
        self.0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ExtentFlags — attributs d'un Extent
// ─────────────────────────────────────────────────────────────────────────────

/// Flags d'un Extent (u8).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct ExtentFlags(pub u8);

impl ExtentFlags {
    /// L'extent est sparse (trou de fichier, zéros logiques).
    pub const SPARSE: Self = Self(1 << 0);
    /// L'extent est compressé inline.
    pub const COMPRESSED: Self = Self(1 << 1);
    /// L'extent appartient à un snapshot (lecture seule).
    pub const SNAPSHOT_RO: Self = Self(1 << 2);
    /// L'extent est un bloc de données chiffrées.
    pub const ENCRYPTED: Self = Self(1 << 3);
    /// L'extent est partagé par CoW (ref_count > 1).
    pub const SHARED_COW: Self = Self(1 << 4);
    /// L'extent est aligné sur 4 KiB (optimisation NVMe).
    pub const ALIGNED_4K: Self = Self(1 << 5);
    // Bits 6-7 : réservés.

    pub const ALL_KNOWN: Self = Self(0x3F);

    pub fn from_bits_validated(bits: u8) -> Result<Self, ExofsError> {
        if bits & !Self::ALL_KNOWN.0 != 0 {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(Self(bits))
    }

    /// Vrai si cet extent n'a pas de données physiques (zero range).
    #[inline]
    pub fn is_virtual(self) -> bool {
        self.contains(Self::SPARSE)
    }
}

impl_flags_common!(ExtentFlags, u8);

impl BitOr for ExtentFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitAnd for ExtentFlags {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// EpochFlags — attributs d'un EpochRecord ou EpochRoot
// ─────────────────────────────────────────────────────────────────────────────

/// Flags d'un Epoch (u16, dans EpochRecord on-disk).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct EpochFlags(pub u16);

impl EpochFlags {
    /// L'epoch est un snapshot permanent (jamais collecté par le GC).
    pub const SNAPSHOT: Self = Self(1 << 0);
    /// L'epoch a été committé avec les 3 barrières NVMe complètes.
    pub const COMMITTED: Self = Self(1 << 1);
    /// L'epoch est en recovery (rejoué au boot).
    pub const RECOVERING: Self = Self(1 << 2);
    /// L'epoch contient des opérations de suppression.
    pub const HAS_DELETIONS: Self = Self(1 << 3);
    /// L'epoch contient des créations de relation.
    pub const HAS_RELATIONS: Self = Self(1 << 4);
    /// L'epoch contient des opérations de migration de tier.
    pub const HAS_MIGRATIONS: Self = Self(1 << 5);
    /// L'epoch résulte d'une opération de déduplication.
    pub const DEDUP_EPOCH: Self = Self(1 << 6);
    /// L'epoch est marqué à vérifier par fsck.
    pub const FSCK_PENDING: Self = Self(1 << 7);
    /// L'epoch contient des mises à jour de quotas.
    pub const HAS_QUOTA_OPS: Self = Self(1 << 8);
    /// L'epoch a été vérifié par le vérificateur en ligne.
    pub const VERIFIED: Self = Self(1 << 9);
    // Bits 10-15 : réservés.

    pub const ALL_KNOWN: Self = Self(0x03FF);
    /// Epoch commité de force (e.g., lors d'une urgence).
    pub const FORCE_COMMITTED: Self = Self(1 << 10);

    pub fn from_bits_validated(bits: u16) -> Result<Self, ExofsError> {
        if bits & !Self::ALL_KNOWN.0 != 0 {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(Self(bits))
    }

    /// Vrai si l'epoch ne doit jamais être collecté par le GC.
    #[inline]
    pub fn is_permanent(self) -> bool {
        self.contains(Self::SNAPSHOT)
    }

    /// Vrai si l'epoch est stable (committé et non en recovery).
    #[inline]
    pub fn is_stable(self) -> bool {
        self.contains(Self::COMMITTED) && !self.contains(Self::RECOVERING)
    }

    /// Fusionne deux sets de flags (union).
    pub fn merge(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl_flags_common!(EpochFlags, u16);

impl BitOr for EpochFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitAnd for EpochFlags {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl BitOrAssign for EpochFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SnapshotFlags — attributs spécifiques aux snapshots
// ─────────────────────────────────────────────────────────────────────────────

/// Flags d'un Snapshot ExoFS (u16).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct SnapshotFlags(pub u16);

impl SnapshotFlags {
    /// Snapshot automatique (créé par le GC scheduler).
    pub const AUTO: Self = Self(1 << 0);
    /// Snapshot créé par l'utilisateur (tag explicite).
    pub const USER: Self = Self(1 << 1);
    /// Snapshot vérifié (tous les blobs accessibles et cohérents).
    pub const VERIFIED: Self = Self(1 << 2);
    /// Snapshot en cours de vérification.
    pub const VERIFYING: Self = Self(1 << 3);
    /// Snapshot exporté vers un archive off-site.
    pub const EXPORTED: Self = Self(1 << 4);
    /// Snapshot marqué pour suppression différée.
    pub const PENDING_DELETE: Self = Self(1 << 5);
    /// Snapshot basé sur un snapshot parent (snapshot incrémental).
    pub const INCREMENTAL: Self = Self(1 << 6);
    /// Snapshot complet (toutes les données, pas de dépendance parent).
    pub const FULL: Self = Self(1 << 7);
    // Bits 8-15 : réservés.

    pub const ALL_KNOWN: Self = Self(0x00FF);

    pub fn from_bits_validated(bits: u16) -> Result<Self, ExofsError> {
        if bits & !Self::ALL_KNOWN.0 != 0 {
            return Err(ExofsError::InvalidArgument);
        }
        // FULL et INCREMENTAL sont mutuellement exclusifs.
        let s = Self(bits);
        if s.contains(Self::FULL) && s.contains(Self::INCREMENTAL) {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(s)
    }
}

impl_flags_common!(SnapshotFlags, u16);

impl BitOr for SnapshotFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MigrationFlags — opérations de migration inter-tiers
// ─────────────────────────────────────────────────────────────────────────────

/// Flags d'une opération de migration de tier (u8).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct MigrationFlags(pub u8);

impl MigrationFlags {
    /// Migration vers tier chaud (SSD/NVMe).
    pub const TO_HOT: Self = Self(1 << 0);
    /// Migration vers tier froid (HDD/Archive).
    pub const TO_COLD: Self = Self(1 << 1);
    /// Migration déclenchée manuellement.
    pub const MANUAL: Self = Self(1 << 2);
    /// Migration planifiée par le scheduler de tiers.
    pub const SCHEDULED: Self = Self(1 << 3);
    /// Migration en cours.
    pub const IN_PROGRESS: Self = Self(1 << 4);
    /// Migration terminée avec succès.
    pub const COMPLETED: Self = Self(1 << 5);
    /// Migration abandonnée (espace cible insuffisant).
    pub const ABORTED: Self = Self(1 << 6);

    pub const ALL_KNOWN: Self = Self(0x7F);

    pub fn is_in_progress(self) -> bool {
        self.contains(Self::IN_PROGRESS)
    }
    pub fn is_done(self) -> bool {
        self.contains(Self::COMPLETED) | self.contains(Self::ABORTED)
    }
}

impl_flags_common!(MigrationFlags, u8);

// ─────────────────────────────────────────────────────────────────────────────
// MountFlags — options de montage runtime
// ─────────────────────────────────────────────────────────────────────────────

/// Flags de montage ExoFS (u32).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct MountFlags(pub u32);

impl MountFlags {
    /// Montage en lecture seule.
    pub const READ_ONLY: Self = Self(1 << 0);
    /// Désactive le writeback journal.
    pub const NO_JOURNAL: Self = Self(1 << 1);
    /// Mode dégradé — montage malgré corruption mineure.
    pub const DEGRADED: Self = Self(1 << 2);
    /// Désactive la déduplication au montage.
    pub const NO_DEDUP: Self = Self(1 << 3);
    /// Désactive la compression au montage.
    pub const NO_COMPRESS: Self = Self(1 << 4);
    /// Vérifie tous les checksums en lecture (mode paranoïde).
    pub const VERIFY_ALL: Self = Self(1 << 5);
    /// Active le mode debug (logs verbeux).
    pub const DEBUG: Self = Self(1 << 6);
    /// Désactive le GC automatique.
    pub const NO_GC: Self = Self(1 << 7);
    /// Montage depuis un snapshot (lecture seule forcée).
    pub const SNAPSHOT_MOUNT: Self = Self(1 << 8);
    /// Désactive le cache de chemins (pour tests).
    pub const NO_PATH_CACHE: Self = Self(1 << 9);

    pub const ALL_KNOWN: Self = Self(0x03FF);

    pub fn from_bits_validated(bits: u32) -> Result<Self, ExofsError> {
        if bits & !Self::ALL_KNOWN.0 != 0 {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(Self(bits))
    }

    /// Vrai si le système peut être monté en écriture.
    #[inline]
    pub fn is_writable(self) -> bool {
        !self.contains(Self::READ_ONLY) && !self.contains(Self::SNAPSHOT_MOUNT)
    }

    /// Profil "performance" : désactive les vérifications non critiques.
    pub const PERFORMANCE: Self = Self(Self::NO_JOURNAL.0 | Self::NO_GC.0 | Self::NO_PATH_CACHE.0);

    /// Profil "sécurité" : active toutes les vérifications.
    pub const SAFETY: Self = Self(Self::VERIFY_ALL.0);
}

impl_flags_common!(MountFlags, u32);

impl BitOr for MountFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for MountFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Utilitaires de validation de flags combinés
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie que la combinaison ObjectFlags + EpochFlags est cohérente.
///
/// Règle : IMMUTABLE + COW_PENDING sont mutuellement exclusifs.
/// Règle : VERIFIED + FSCK_PENDING sont mutuellement exclusifs.
pub fn validate_object_epoch_flags(obj_flags: ObjectFlags, epoch_flags: EpochFlags) -> bool {
    // Un objet immuable ne peut pas avoir un CoW en attente.
    if obj_flags.0 & ObjectFlags::IMMUTABLE.0 != 0 && obj_flags.0 & ObjectFlags::COW_PENDING.0 != 0
    {
        return false;
    }
    // VERIFIED et FSCK_PENDING sont mutuellement exclusifs.
    let verified = EpochFlags::VERIFIED.0;
    let fsck_pend = EpochFlags::FSCK_PENDING.0;
    if epoch_flags.0 & (verified | fsck_pend) == (verified | fsck_pend) {
        return false;
    }
    true
}

/// Crée des ObjectFlags de départ pour un objet Class1 fraîchement créé.
pub fn initial_class1_flags() -> ObjectFlags {
    ObjectFlags::VERIFIED | ObjectFlags::IMMUTABLE
}

/// Crée des ObjectFlags de départ pour un objet Class2 fraîchement créé.
pub fn initial_class2_flags() -> ObjectFlags {
    ObjectFlags::NEEDS_SYNC
}

/// Retourne vrai si un objet peut être inclus dans un snapshot.
pub fn snapshot_eligible(obj: ObjectFlags, snap: SnapshotFlags) -> bool {
    obj.0 & ObjectFlags::DELETED.0 == 0 && snap.0 & SnapshotFlags::PENDING_DELETE.0 == 0
}
