// kernel/src/security/zero_trust/labels.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// SECURITY LABELS — Labels de sécurité MLS-like (Zero-Trust)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Système de labels de sécurité inspiré de Bell-LaPadula (confidentialité)
// et Biba (intégrité). Simplifié pour un OS embarqué sans base de données MLS.
//
// MODÈLE :
//   Confidentialité : [0=Public, 1=Internal, 2=Confidential, 3=Secret, 4=TopSecret]
//   Intégrité       : [0=Untrusted, 1=Low, 2=Medium, 3=High, 4=Critical]
//
// REGLES Bell-LaPadula (confidentialité) :
//   No-read-up   : sujet ne peut lire un objet de niveau supérieur
//   No-write-down: sujet ne peut écrire dans un objet de niveau inférieur
//
// REGLES Biba (intégrité) :
//   No-read-down : sujet ne peut lire un objet d'intégrité inférieure
//   No-write-up  : sujet ne peut écrire dans un objet d'intégrité supérieure
// ═══════════════════════════════════════════════════════════════════════════════

// ─────────────────────────────────────────────────────────────────────────────
// ConfidentialityLevel
// ─────────────────────────────────────────────────────────────────────────────

/// Niveau de confidentialité — classification des données.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum ConfidentialityLevel {
    Public = 0,
    Internal = 1,
    Confidential = 2,
    Secret = 3,
    TopSecret = 4,
}

impl ConfidentialityLevel {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Public,
            1 => Self::Internal,
            2 => Self::Confidential,
            3 => Self::Secret,
            4 => Self::TopSecret,
            _ => Self::Public,
        }
    }

    /// Bell-LaPadula no-read-up : sujet ne peut lire un objet de niveau supérieur.
    #[inline(always)]
    pub fn can_read(subject: Self, object: Self) -> bool {
        subject >= object
    }

    /// Bell-LaPadula no-write-down.
    #[inline(always)]
    pub fn can_write(subject: Self, object: Self) -> bool {
        subject <= object
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// IntegrityLevel
// ─────────────────────────────────────────────────────────────────────────────

/// Niveau d'intégrité — fiabilité de la source de données.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum IntegrityLevel {
    Untrusted = 0,
    Low = 1,
    Medium = 2,
    High = 3,
    Critical = 4,
}

impl IntegrityLevel {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Untrusted,
            1 => Self::Low,
            2 => Self::Medium,
            3 => Self::High,
            4 => Self::Critical,
            _ => Self::Untrusted,
        }
    }

    /// Biba no-read-down : ne pas lire des données de moins bonne intégrité.
    #[inline(always)]
    pub fn can_read(subject: Self, object: Self) -> bool {
        subject <= object
    }

    /// Biba no-write-up : ne pas écrire dans un objet d'intégrité supérieure.
    #[inline(always)]
    pub fn can_write(subject: Self, object: Self) -> bool {
        subject >= object
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SecurityLabel — combinaison confidentialité + intégrité
// ─────────────────────────────────────────────────────────────────────────────

/// Label de sécurité composite : confidentialité × intégrité.
/// Taille : 2 bytes — peut être stocké dans le TCB sans impact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct SecurityLabel {
    pub confidentiality: ConfidentialityLevel,
    pub integrity: IntegrityLevel,
}

impl SecurityLabel {
    /// Label noyau : TopSecret + Critical intégrité.
    pub const fn kernel() -> Self {
        Self {
            confidentiality: ConfidentialityLevel::TopSecret,
            integrity: IntegrityLevel::Critical,
        }
    }

    /// Label utilisateur par défaut : Internal + Medium.
    pub const fn user_default() -> Self {
        Self {
            confidentiality: ConfidentialityLevel::Internal,
            integrity: IntegrityLevel::Medium,
        }
    }

    /// Label public : Public + Low intégrité.
    pub const fn public() -> Self {
        Self {
            confidentiality: ConfidentialityLevel::Public,
            integrity: IntegrityLevel::Low,
        }
    }

    /// Label pour un driver système : Confidential + High.
    pub const fn driver() -> Self {
        Self {
            confidentiality: ConfidentialityLevel::Confidential,
            integrity: IntegrityLevel::High,
        }
    }

    /// Crée un label avec des niveaux spécifiés.
    pub const fn new(c: ConfidentialityLevel, i: IntegrityLevel) -> Self {
        Self {
            confidentiality: c,
            integrity: i,
        }
    }

    /// Vérifie si ce sujet (self) peut LIRE un objet avec le label `object`.
    /// Combine Bell-LaPadula + Biba.
    #[inline(always)]
    pub fn can_read(self, object: SecurityLabel) -> bool {
        ConfidentialityLevel::can_read(self.confidentiality, object.confidentiality)
            && IntegrityLevel::can_read(self.integrity, object.integrity)
    }

    /// Vérifie si ce sujet peut ÉCRIRE dans un objet avec le label `object`.
    #[inline(always)]
    pub fn can_write(self, object: SecurityLabel) -> bool {
        ConfidentialityLevel::can_write(self.confidentiality, object.confidentiality)
            && IntegrityLevel::can_write(self.integrity, object.integrity)
    }

    /// Domination : self domine object (≥ sur les deux axes).
    #[inline(always)]
    pub fn dominates(self, other: SecurityLabel) -> bool {
        self.confidentiality >= other.confidentiality && self.integrity >= other.integrity
    }

    /// Label hérité pour un fils — conservateur (prend le minimum).
    pub fn inherit(self) -> Self {
        // L'enfant prend le minimum sur les deux axes (principe du moindre privilège)
        Self {
            confidentiality: match self.confidentiality {
                ConfidentialityLevel::TopSecret => ConfidentialityLevel::Secret,
                other => other,
            },
            integrity: match self.integrity {
                IntegrityLevel::Critical => IntegrityLevel::High,
                other => other,
            },
        }
    }

    /// Encodage compact en u16 pour stockage.
    pub fn encode(self) -> u16 {
        ((self.confidentiality as u16) << 8) | (self.integrity as u16)
    }

    /// Décode depuis u16.
    pub fn decode(v: u16) -> Self {
        Self {
            confidentiality: ConfidentialityLevel::from_u8((v >> 8) as u8),
            integrity: IntegrityLevel::from_u8((v & 0xFF) as u8),
        }
    }
}

impl Default for SecurityLabel {
    fn default() -> Self {
        Self::user_default()
    }
}
