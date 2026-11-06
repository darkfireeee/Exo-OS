// kernel/src/security/zero_trust/labels.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// SECURITY LABELS — MLS-like security labels (Zero-Trust)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Security label system inspired by Bell-LaPadula (confidentiality)
// and Biba (integrity). Simplified for an embedded OS without MLS database.
//
// MODEL:
//   Confidentiality: [0=Public, 1=Internal, 2=Confidential, 3=Secret, 4=TopSecret]
//   Integrity      : [0=Untrusted, 1=Low, 2=Medium, 3=High, 4=Critical]
//
// Bell-LaPadula RULES (confidentiality):
//   No-read-up   : subject cannot read an object of higher level
//   No-write-down: subject cannot write to an object of lower level
//
// Biba RULES (integrity):
//   No-read-down : subject cannot read an object of lower integrity
//   No-write-up  : subject cannot write to an object of higher integrity
// ═══════════════════════════════════════════════════════════════════════════════

// ─────────────────────────────────────────────────────────────────────────────
// ConfidentialityLevel
// ─────────────────────────────────────────────────────────────────────────────

/// Confidentiality level — data classification.
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

    /// Bell-LaPadula no-read-up: subject cannot read an object of higher level.
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

/// Integrity level — data source trustworthiness.
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

    /// Biba no-read-down: do not read data of lower integrity.
    #[inline(always)]
    pub fn can_read(subject: Self, object: Self) -> bool {
        subject <= object
    }

    /// Biba no-write-up: do not write to an object of higher integrity.
    #[inline(always)]
    pub fn can_write(subject: Self, object: Self) -> bool {
        subject >= object
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SecurityLabel — combined confidentiality + integrity
// ─────────────────────────────────────────────────────────────────────────────

/// Composite security label: confidentiality × integrity.
/// Size: 2 bytes — can be stored in the TCB without overhead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct SecurityLabel {
    pub confidentiality: ConfidentialityLevel,
    pub integrity: IntegrityLevel,
}

impl SecurityLabel {
    /// Kernel label: TopSecret + Critical integrity.
    pub const fn kernel() -> Self {
        Self {
            confidentiality: ConfidentialityLevel::TopSecret,
            integrity: IntegrityLevel::Critical,
        }
    }

    /// Default user label: Internal + Medium.
    pub const fn user_default() -> Self {
        Self {
            confidentiality: ConfidentialityLevel::Internal,
            integrity: IntegrityLevel::Medium,
        }
    }

    /// Public label: Public + Low integrity.
    pub const fn public() -> Self {
        Self {
            confidentiality: ConfidentialityLevel::Public,
            integrity: IntegrityLevel::Low,
        }
    }

    /// Label for a system driver: Confidential + High.
    pub const fn driver() -> Self {
        Self {
            confidentiality: ConfidentialityLevel::Confidential,
            integrity: IntegrityLevel::High,
        }
    }

    /// Creates a label with specified levels.
    pub const fn new(c: ConfidentialityLevel, i: IntegrityLevel) -> Self {
        Self {
            confidentiality: c,
            integrity: i,
        }
    }

    /// Checks if this subject (self) can READ an object with label `object`.
    /// Combines Bell-LaPadula + Biba.
    #[inline(always)]
    pub fn can_read(self, object: SecurityLabel) -> bool {
        ConfidentialityLevel::can_read(self.confidentiality, object.confidentiality)
            && IntegrityLevel::can_read(self.integrity, object.integrity)
    }

    /// Checks if this subject can WRITE to an object with label `object`.
    #[inline(always)]
    pub fn can_write(self, object: SecurityLabel) -> bool {
        ConfidentialityLevel::can_write(self.confidentiality, object.confidentiality)
            && IntegrityLevel::can_write(self.integrity, object.integrity)
    }

    /// Domination: self dominates object (≥ on both axes).
    #[inline(always)]
    pub fn dominates(self, other: SecurityLabel) -> bool {
        self.confidentiality >= other.confidentiality && self.integrity >= other.integrity
    }

    /// Inherited label for a child — conservative (takes the minimum).
    pub fn inherit(self) -> Self {
        // Child takes the minimum on both axes (principle of least privilege)
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

    /// Compact encoding into u16 for storage.
    pub fn encode(self) -> u16 {
        ((self.confidentiality as u16) << 8) | (self.integrity as u16)
    }

    /// Decodes from u16.
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
