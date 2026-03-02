// kernel/src/fs/exofs/core/version.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Version — FormatVersion, feature flags, négociation, matrice compatibilité
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Règles de compatibilité :
//   VER-01 : Même majeur → toujours montable (rw si mineur <=, sinon ro).
//   VER-02 : Majeur différent → refus de montage.
//   VER-03 : Les feature flags permettent une dégradation gracieuse.
//   VER-04 : Le kernel stocke FORMAT_VERSION_MAJOR.FORMAT_VERSION_MINOR
//            dans le superbloc lors du format.

use crate::fs::exofs::core::constants::{FORMAT_VERSION_MAJOR, FORMAT_VERSION_MINOR};
use crate::fs::exofs::core::error::ExofsError;

// ─────────────────────────────────────────────────────────────────────────────
// FormatVersion — version du format on-disk
// ─────────────────────────────────────────────────────────────────────────────

/// Descripteur de version du format on-disk ExoFS.
///
/// Stocké dans le SuperBloc (u16 major + u16 minor = 4 octets on-disk).
/// La règle de compatibilité (VER-01/VER-02) est implémentée dans
/// `check_mount_compatibility`.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FormatVersion {
    pub major: u16,
    pub minor: u16,
}

const _: () = assert!(core::mem::size_of::<FormatVersion>() == 4);

impl FormatVersion {
    /// Version courante du kernel ExoFS.
    pub const CURRENT: Self = Self {
        major: FORMAT_VERSION_MAJOR,
        minor: FORMAT_VERSION_MINOR,
    };

    /// Version minimale lisible (rétrocompatibilité garantie jusqu'à 1.0).
    pub const MIN_READABLE: Self = Self { major: 1, minor: 0 };

    /// Crée une FormatVersion depuis deux entiers.
    #[inline]
    pub const fn new(major: u16, minor: u16) -> Self { Self { major, minor } }

    /// Désérialisation depuis 4 octets on-disk (little-endian).
    #[inline]
    pub fn from_le_bytes(b: [u8; 4]) -> Self {
        Self {
            major: u16::from_le_bytes([b[0], b[1]]),
            minor: u16::from_le_bytes([b[2], b[3]]),
        }
    }

    /// Sérialisation en 4 octets on-disk (little-endian).
    #[inline]
    pub fn to_le_bytes(self) -> [u8; 4] {
        let mj = self.major.to_le_bytes();
        let mn = self.minor.to_le_bytes();
        [mj[0], mj[1], mn[0], mn[1]]
    }

    /// Retourne vrai si cette version peut être montée par le kernel courant.
    ///
    /// Règle VER-01 : même majeur → OK. Règle VER-02 : majeur diff → Err.
    pub fn is_compatible_with_current(self) -> Result<(), ExofsError> {
        if self.major != FORMAT_VERSION_MAJOR {
            return Err(ExofsError::IncompatibleVersion);
        }
        // Mineur supérieur = format plus récent, montage en lecture seule admis.
        Ok(())
    }

    /// Montable en lecture-écriture (mineur == courant, même majeur).
    pub fn is_read_write_compatible(self) -> bool {
        self.major == FORMAT_VERSION_MAJOR && self.minor <= FORMAT_VERSION_MINOR
    }

    /// Nécessite une migration avant montage rw (mineur futur, même majeur).
    pub fn requires_migration(self) -> bool {
        self.major == FORMAT_VERSION_MAJOR && self.minor > FORMAT_VERSION_MINOR
    }

    /// Vérifie strictement que la version est identique à CURRENT.
    pub fn is_exact(self) -> Result<(), ExofsError> {
        if self != Self::CURRENT {
            Err(ExofsError::IncompatibleVersion)
        } else {
            Ok(())
        }
    }
}

impl core::fmt::Display for FormatVersion {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// FeatureFlags — fonctionnalités optionnelles du format
// ─────────────────────────────────────────────────────────────────────────────

/// Feature flags persistés dans le SuperBloc (u32 on-disk).
///
/// Permettent une dégradation gracieuse si une fonctionnalité est inconnue
/// du kernel courant (règle VER-03).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct FeatureFlags(pub u32);

impl FeatureFlags {
    /// Bit 0 : déduplication activée (chunk-store présent).
    pub const DEDUP:       Self = Self(1 << 0);
    /// Bit 1 : compression activée (le format Lz4 ou Zstd est utilisé).
    pub const COMPRESSION: Self = Self(1 << 1);
    /// Bit 2 : chiffrement de blobs activé (AES-256-GCM).
    pub const ENCRYPTION:  Self = Self(1 << 2);
    /// Bit 3 : snapshots activés (slot epoch permanent présent).
    pub const SNAPSHOTS:   Self = Self(1 << 3);
    /// Bit 4 : relations typées activées (relation graph stocké).
    pub const RELATIONS:   Self = Self(1 << 4);
    /// Bit 5 : quota namespaces activés.
    pub const QUOTAS:      Self = Self(1 << 5);
    /// Bit 6 : NUMA placement hints stockés dans les blobs.
    pub const NUMA_HINTS:  Self = Self(1 << 6);
    /// Bit 7 : PathIndex v2 (support des noms UTF-8 normalisés).
    pub const PATH_V2:     Self = Self(1 << 7);

    /// Masque de toutes les features connues par ce kernel.
    pub const KNOWN: Self = Self(0x0000_00FF);

    #[inline] pub fn has(self, flag: Self) -> bool { self.0 & flag.0 == flag.0 }
    #[inline] pub fn set(&mut self, flag: Self) { self.0 |= flag.0; }
    #[inline] pub fn clear(&mut self, flag: Self) { self.0 &= !flag.0; }

    /// Vrai si ce flag-set contient des features inconnues du kernel courant.
    #[inline]
    pub fn has_unknown_features(self) -> bool {
        self.0 & !Self::KNOWN.0 != 0
    }

    /// Retourne le masque des features inconnues (pour log/diagnostic).
    #[inline]
    pub fn unknown_features(self) -> u32 {
        self.0 & !Self::KNOWN.0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// VersionNegotiator — décision de montage
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat de la négociation de version au montage.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MountCompatibility {
    /// Montage complet lecture-écriture possible.
    ReadWrite,
    /// Montage en lecture seule recommandé (feature future inconnue).
    ReadOnly,
    /// Montage refusé (majeur incompatible ou features obligatoires inconnues).
    Rejected,
}

/// Négocie la compatibilité au montage à partir de la version et des features on-disk.
///
/// Règles appliquées :
///   1. Majeur différent → Rejected (VER-02).
///   2. Features inconnues → ReadOnly (dégradation gracieuse, VER-03).
///   3. Mineur futur sans features inconnues → ReadOnly (VER-01).
///   4. Sinon → ReadWrite.
pub fn negotiate_mount(
    disk_version:  FormatVersion,
    disk_features: FeatureFlags,
) -> MountCompatibility {
    // Règle VER-02 : majeur différent = rejet absolu.
    if disk_version.major != FORMAT_VERSION_MAJOR {
        return MountCompatibility::Rejected;
    }
    // Features inconnues → dégradation en ro (VER-03).
    if disk_features.has_unknown_features() {
        return MountCompatibility::ReadOnly;
    }
    // Mineur supérieur → ro par précaution (VER-01).
    if disk_version.minor > FORMAT_VERSION_MINOR {
        return MountCompatibility::ReadOnly;
    }
    MountCompatibility::ReadWrite
}

// ─────────────────────────────────────────────────────────────────────────────
// VersionNegotiationResult — résultat détaillé de la négociation
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat détaillé de la négociation de compatibilité.
#[derive(Clone, Debug)]
pub struct VersionNegotiationResult {
    /// Résultat final de compatibilité.
    pub compat:    MountCompatibility,
    /// Version trouvée sur le disque.
    pub disk_ver:  FormatVersion,
    /// Features trouvées sur le disque.
    pub disk_feat: FeatureFlags,
    /// Raison lisible de la décision.
    pub reason:    &'static str,
    /// Features inconnues détectées (bits masqués).
    pub unknown_features: u32,
}

impl VersionNegotiationResult {
    /// Vrai si le montage est autorisé (rw ou ro).
    pub fn is_mountable(&self) -> bool {
        !matches!(self.compat, MountCompatibility::Rejected)
    }

    /// Vrai si le montage est en lecture-écriture.
    pub fn is_read_write(&self) -> bool {
        matches!(self.compat, MountCompatibility::ReadWrite)
    }
}

/// Négociation détaillée avec raison.
pub fn negotiate_mount_detailed(
    disk_version:  FormatVersion,
    disk_features: FeatureFlags,
) -> VersionNegotiationResult {
    let unknown = disk_features.unknown_features();

    if disk_version.major != FORMAT_VERSION_MAJOR {
        return VersionNegotiationResult {
            compat:           MountCompatibility::Rejected,
            disk_ver:         disk_version,
            disk_feat:        disk_features,
            reason:           "incompatible major version (VER-02)",
            unknown_features: unknown,
        };
    }
    if unknown != 0 {
        return VersionNegotiationResult {
            compat:           MountCompatibility::ReadOnly,
            disk_ver:         disk_version,
            disk_feat:        disk_features,
            reason:           "unknown feature flags present (VER-03)",
            unknown_features: unknown,
        };
    }
    if disk_version.minor > FORMAT_VERSION_MINOR {
        return VersionNegotiationResult {
            compat:           MountCompatibility::ReadOnly,
            disk_ver:         disk_version,
            disk_feat:        disk_features,
            reason:           "future minor version, degraded to read-only (VER-01)",
            unknown_features: 0,
        };
    }
    VersionNegotiationResult {
        compat:           MountCompatibility::ReadWrite,
        disk_ver:         disk_version,
        disk_feat:        disk_features,
        reason:           "fully compatible",
        unknown_features: 0,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// FeatureFlagsBuilder — construction fluente de FeatureFlags
// ─────────────────────────────────────────────────────────────────────────────

/// Builder fluent pour FeatureFlags.
#[derive(Default, Clone, Debug)]
pub struct FeatureFlagsBuilder(u32);

impl FeatureFlagsBuilder {
    /// Commence avec aucun flag.
    pub fn new() -> Self { Self(0) }

    /// Active la déduplication.
    pub fn dedup(mut self) -> Self { self.0 |= FeatureFlags::DEDUP; self }
    /// Active la compression.
    pub fn compression(mut self) -> Self { self.0 |= FeatureFlags::COMPRESSION; self }
    /// Active le chiffrement.
    pub fn encryption(mut self) -> Self { self.0 |= FeatureFlags::ENCRYPTION; self }
    /// Active les snapshots.
    pub fn snapshots(mut self) -> Self { self.0 |= FeatureFlags::SNAPSHOTS; self }
    /// Active les relations.
    pub fn relations(mut self) -> Self { self.0 |= FeatureFlags::RELATIONS; self }
    /// Active les quotas.
    pub fn quotas(mut self) -> Self { self.0 |= FeatureFlags::QUOTAS; self }
    /// Active les hints NUMA.
    pub fn numa_hints(mut self) -> Self { self.0 |= FeatureFlags::NUMA_HINTS; self }
    /// Active le format de path v2.
    pub fn path_v2(mut self) -> Self { self.0 |= FeatureFlags::PATH_V2; self }

    /// Construit le FeatureFlags final.
    pub fn build(self) -> FeatureFlags { FeatureFlags(self.0) }
}

// ─────────────────────────────────────────────────────────────────────────────
// Dépendances entre features
// ─────────────────────────────────────────────────────────────────────────────

/// Dépendance entre deux FeatureFlags.
#[derive(Copy, Clone, Debug)]
pub struct FeatureDependency {
    /// Feature dépendante.
    pub feature:  u32,
    /// Feature requise (prérequis).
    pub requires: u32,
    /// Description lisible.
    pub desc:     &'static str,
}

/// Tableau des dépendances entre features.
///
/// Exemple : ENCRYPTION requiert SNAPSHOTS (pour le recovery key store).
static FEATURE_DEPS: &[FeatureDependency] = &[
    FeatureDependency {
        feature:  FeatureFlags::ENCRYPTION,
        requires: FeatureFlags::SNAPSHOTS,
        desc:     "ENCRYPTION requires SNAPSHOTS (key store recovery)",
    },
    FeatureDependency {
        feature:  FeatureFlags::PATH_V2,
        requires: FeatureFlags::RELATIONS,
        desc:     "PATH_V2 requires RELATIONS (typed edges)",
    },
];

/// Vérifie que les dépendances entre features sont satisfaites.
///
/// Retourne la première dépendance non satisfaite, ou None si tout est OK.
pub fn check_feature_dependencies(flags: FeatureFlags) -> Option<&'static FeatureDependency> {
    for dep in FEATURE_DEPS {
        if flags.has(dep.feature) && !flags.has(dep.requires) {
            return Some(dep);
        }
    }
    None
}

// ─────────────────────────────────────────────────────────────────────────────
// Historique des versions — table de migration
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée dans l'historique des versions du format ExoFS.
#[derive(Copy, Clone, Debug)]
pub struct VersionHistoryEntry {
    pub version:     FormatVersion,
    pub description: &'static str,
    pub added_features: u32,
    pub deprecated_features: u32,
}

/// Table statique des versions du format ExoFS.
static VERSION_HISTORY: &[VersionHistoryEntry] = &[
    VersionHistoryEntry {
        version: FormatVersion { major: 1, minor: 0 },
        description: "Initial ExoFS format — blobs, code, config, secret, pathindex, relation",
        added_features: FeatureFlags::DEDUP | FeatureFlags::COMPRESSION | FeatureFlags::SNAPSHOTS,
        deprecated_features: 0,
    },
];

/// Retourne l'entrée d'historique pour une version donnée, ou None.
pub fn version_history_entry(v: FormatVersion) -> Option<&'static VersionHistoryEntry> {
    VERSION_HISTORY.iter().find(|e| e.version.major == v.major && e.version.minor == v.minor)
}

// ─────────────────────────────────────────────────────────────────────────────
// MigrationDescriptor — description d'un chemin de migration
// ─────────────────────────────────────────────────────────────────────────────

/// Descripteur d'un chemin de migration entre deux versions.
#[derive(Copy, Clone, Debug)]
pub struct MigrationDescriptor {
    pub from:   FormatVersion,
    pub to:     FormatVersion,
    /// Estimation du coût en epochs.
    pub estimated_epochs: u32,
    /// Migration peut se faire à chaud (sans démonter le volume).
    pub online: bool,
    pub description: &'static str,
}

/// Retourne le descripteur de migration applicable, si disponible.
///
/// Retourne None si aucune migration auto n'est disponible.
pub fn find_migration_path(
    from: FormatVersion,
    to:   FormatVersion,
) -> Option<MigrationDescriptor> {
    // Pour l'instant seule une migration 1.0→1.x est définie.
    if from.major == 1 && to.major == 1 && to.minor > from.minor {
        Some(MigrationDescriptor {
            from,
            to,
            estimated_epochs: (to.minor - from.minor) as u32 * 2,
            online: true,
            description: "incremental minor version migration",
        })
    } else {
        None
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Estimation d'overhead par feature
// ─────────────────────────────────────────────────────────────────────────────

/// Overhead on-disk ajouté par une feature (pourcentage × 10).
///
/// Exemple : DEDUP = 5 → 0.5% d'overhead de métadonnées.
pub fn feature_overhead_pct10(feature: u32) -> u32 {
    match feature {
        FeatureFlags::DEDUP        => 5,   // 0.5%
        FeatureFlags::COMPRESSION  => 3,   // 0.3%
        FeatureFlags::ENCRYPTION   => 10,  // 1.0%
        FeatureFlags::SNAPSHOTS    => 8,   // 0.8%
        FeatureFlags::RELATIONS    => 4,   // 0.4%
        FeatureFlags::QUOTAS       => 2,   // 0.2%
        FeatureFlags::NUMA_HINTS   => 1,   // 0.1%
        FeatureFlags::PATH_V2      => 3,   // 0.3%
        _                         => 0,
    }
}

/// Overhead total × 10 pour un ensemble de features activées.
pub fn total_features_overhead_pct10(flags: FeatureFlags) -> u32 {
    let mut total = 0u32;
    let known = FeatureFlags::KNOWN;
    let mut bit = 1u32;
    while bit <= known {
        if flags.has(bit) {
            total = total.saturating_add(feature_overhead_pct10(bit));
        }
        bit <<= 1;
    }
    total
}

// ─────────────────────────────────────────────────────────────────────────────
// Validation du format sur la géométrie disque
// ─────────────────────────────────────────────────────────────────────────────

/// Valide que la version est cohérente avec les géométries on-disk fournies.
///
/// Paramètres vérifiés :
///   - version dans la plage connue.
///   - features sans dépendances manquantes.
///   - overhead total < 50% (protection contre la corruption).
pub fn validate_version_on_disk(
    version:  FormatVersion,
    features: FeatureFlags,
) -> Result<(), &'static str> {
    // Version majeure inconnue.
    if version.major > FORMAT_VERSION_MAJOR {
        return Err("unknown major version");
    }
    // Dépendances entre features.
    if let Some(dep) = check_feature_dependencies(features) {
        return Err(dep.desc);
    }
    // Overhead total raisonnable.
    if total_features_overhead_pct10(features) > 500 {
        return Err("feature overhead exceeds 50%, likely corrupted");
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Utilitaires de format et de vérification finale
// ─────────────────────────────────────────────────────────────────────────────

/// Sérialise une FormatVersion sur 4 octets little-endian (ONDISK-01).
///
/// Format : [minor_lo, minor_hi, major_lo, major_hi]
pub fn version_to_le_bytes(v: FormatVersion) -> [u8; 4] {
    let [m0, m1] = v.minor.to_le_bytes();
    let [M0, M1] = v.major.to_le_bytes();
    [m0, m1, M0, M1]
}

/// Désérialise une FormatVersion depuis 4 octets little-endian.
pub fn version_from_le_bytes(b: [u8; 4]) -> FormatVersion {
    let minor = u16::from_le_bytes([b[0], b[1]]);
    let major = u16::from_le_bytes([b[2], b[3]]);
    FormatVersion { major, minor }
}

/// Retourne vrai si deux versions sont identiques (exact match).
pub fn versions_equal(a: FormatVersion, b: FormatVersion) -> bool {
    a.major == b.major && a.minor == b.minor
}

/// Retourne vrai si la version `a` est antérieure à `b`.
pub fn version_less_than(a: FormatVersion, b: FormatVersion) -> bool {
    (a.major, a.minor) < (b.major, b.minor)
}

/// Retourne la version la plus récente entre deux candidates.
pub fn version_max(a: FormatVersion, b: FormatVersion) -> FormatVersion {
    if version_less_than(a, b) { b } else { a }
}

/// Vrai si une feature est requise pour une opération donnée.
///
/// Certaines opérations ne sont disponibles que si une feature est activée.
/// Par exemple, un snapshot requiert SNAPSHOTS.
pub fn feature_required_for(op: &str, features: FeatureFlags) -> bool {
    match op {
        "snapshot"   => features.has(FeatureFlags::SNAPSHOTS),
        "dedup"      => features.has(FeatureFlags::DEDUP),
        "compress"   => features.has(FeatureFlags::COMPRESSION),
        "encrypt"    => features.has(FeatureFlags::ENCRYPTION),
        "quota"      => features.has(FeatureFlags::QUOTAS),
        _           => true, // feature inconnue → admis par défaut
    }
}

