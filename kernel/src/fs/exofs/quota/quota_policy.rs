// SPDX-License-Identifier: MIT
// ExoFS Quota — Politique et Limites
// ≥400L, ExofsError only, RECUR-01/OOM-02/ARITH-02

use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─── QuotaKind ────────────────────────────────────────────────────────────────

/// Type d'entité soumise à un quota.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum QuotaKind {
    User    = 0,
    Group   = 1,
    Project = 2,
}

impl QuotaKind {
    pub fn from_u8(v: u8) -> Self {
        match v { 1 => Self::Group, 2 => Self::Project, _ => Self::User }
    }
    pub fn name(self) -> &'static str {
        match self { Self::User => "user", Self::Group => "group", Self::Project => "project" }
    }
    pub const COUNT: usize = 3;
}

// ─── QuotaLimits ─────────────────────────────────────────────────────────────

/// Limites de quota pour une entité (octets, blobs, inodes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuotaLimits {
    /// Limite souple en octets (avertissement).
    pub soft_bytes:  u64,
    /// Limite dure en octets (blocage immédiat).
    pub hard_bytes:  u64,
    /// Limite souple en nombre de blobs.
    pub soft_blobs:  u64,
    /// Limite dure en nombre de blobs.
    pub hard_blobs:  u64,
    /// Limite souple en nombre d'inodes.
    pub soft_inodes: u64,
    /// Limite dure en nombre d'inodes.
    pub hard_inodes: u64,
    /// Durée de grâce après dépassement soft (en ticks).
    pub grace_ticks: u64,
}

impl QuotaLimits {
    /// Quotas illimités (valeur par défaut).
    pub const fn unlimited() -> Self {
        Self {
            soft_bytes:  u64::MAX, hard_bytes:  u64::MAX,
            soft_blobs:  u64::MAX, hard_blobs:  u64::MAX,
            soft_inodes: u64::MAX, hard_inodes: u64::MAX,
            grace_ticks: 0,
        }
    }

    /// Valide les limites : soft ≤ hard pour chaque dimension.
    pub fn validate(&self) -> ExofsResult<()> {
        if self.soft_bytes > self.hard_bytes {
            return Err(ExofsError::InvalidArgument);
        }
        if self.soft_blobs > self.hard_blobs {
            return Err(ExofsError::InvalidArgument);
        }
        if self.soft_inodes > self.hard_inodes {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(())
    }

    /// Vrai si toutes les limites sont à MAX (non contraignant).
    pub fn is_unlimited(&self) -> bool {
        self.hard_bytes  == u64::MAX
        && self.hard_blobs  == u64::MAX
        && self.hard_inodes == u64::MAX
    }

    /// Calcul du pourcentage d'utilisation bytes en ‰ (ARITH-02).
    pub fn bytes_usage_ppt(&self, used: u64) -> u64 {
        if self.hard_bytes == 0 || self.hard_bytes == u64::MAX { return 0; }
        used.saturating_mul(1000)
            .checked_div(self.hard_bytes).unwrap_or(1000)
            .min(1000)
    }

    /// Calcul du pourcentage d'utilisation blobs en ‰.
    pub fn blobs_usage_ppt(&self, used: u64) -> u64 {
        if self.hard_blobs == 0 || self.hard_blobs == u64::MAX { return 0; }
        used.saturating_mul(1000)
            .checked_div(self.hard_blobs).unwrap_or(1000)
            .min(1000)
    }

    /// Calcul du pourcentage d'utilisation inodes en ‰.
    pub fn inodes_usage_ppt(&self, used: u64) -> u64 {
        if self.hard_inodes == 0 || self.hard_inodes == u64::MAX { return 0; }
        used.saturating_mul(1000)
            .checked_div(self.hard_inodes).unwrap_or(1000)
            .min(1000)
    }

    /// Vrai si la limite dure bytes est dépassée.
    pub fn bytes_hard_exceeded(&self, used: u64) -> bool {
        self.hard_bytes != u64::MAX && used >= self.hard_bytes
    }

    /// Vrai si la limite souple bytes est dépassée.
    pub fn bytes_soft_exceeded(&self, used: u64) -> bool {
        self.soft_bytes != u64::MAX && used >= self.soft_bytes
    }

    /// Vrai si la limite dure blobs est dépassée.
    pub fn blobs_hard_exceeded(&self, used: u64) -> bool {
        self.hard_blobs != u64::MAX && used >= self.hard_blobs
    }

    /// Vrai si la limite souple blobs est dépassée.
    pub fn blobs_soft_exceeded(&self, used: u64) -> bool {
        self.soft_blobs != u64::MAX && used >= self.soft_blobs
    }

    /// Vrai si la limite dure inodes est dépassée.
    pub fn inodes_hard_exceeded(&self, used: u64) -> bool {
        self.hard_inodes != u64::MAX && used >= self.hard_inodes
    }

    /// Vrai si la limite souple inodes est dépassée.
    pub fn inodes_soft_exceeded(&self, used: u64) -> bool {
        self.soft_inodes != u64::MAX && used >= self.soft_inodes
    }

    /// Octets disponibles avant la limite dure (saturating).
    pub fn bytes_remaining(&self, used: u64) -> u64 {
        if self.hard_bytes == u64::MAX { return u64::MAX; }
        self.hard_bytes.saturating_sub(used)
    }

    /// Blobs disponibles avant la limite dure.
    pub fn blobs_remaining(&self, used: u64) -> u64 {
        if self.hard_blobs == u64::MAX { return u64::MAX; }
        self.hard_blobs.saturating_sub(used)
    }

    /// Inodes disponibles avant la limite dure.
    pub fn inodes_remaining(&self, used: u64) -> u64 {
        if self.hard_inodes == u64::MAX { return u64::MAX; }
        self.hard_inodes.saturating_sub(used)
    }
}

// ─── PolicyFlags ─────────────────────────────────────────────────────────────

/// Drapeaux de comportement d'une politique de quota.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PolicyFlags(pub u32);

impl PolicyFlags {
    /// Politique activée.
    pub const ENABLED:      PolicyFlags = PolicyFlags(1 << 0);
    /// Journaliser les dépassements soft.
    pub const LOG_SOFT:     PolicyFlags = PolicyFlags(1 << 1);
    /// Journaliser les refus hard.
    pub const LOG_HARD:     PolicyFlags = PolicyFlags(1 << 2);
    /// Envoyer une alerte lors d'un dépassement soft.
    pub const ALERT_SOFT:   PolicyFlags = PolicyFlags(1 << 3);
    /// Héritage des limites depuis le parent namespace.
    pub const INHERIT:      PolicyFlags = PolicyFlags(1 << 4);
    /// Mode strict : soft = hard (pas de grâce).
    pub const STRICT:       PolicyFlags = PolicyFlags(1 << 5);
    /// Activer la journalisation des modifications de limites.
    pub const AUDIT_LIMITS: PolicyFlags = PolicyFlags(1 << 6);

    pub const fn default_flags() -> Self {
        // ENABLED | LOG_HARD | ALERT_SOFT
        PolicyFlags(Self::ENABLED.0 | Self::LOG_HARD.0 | Self::ALERT_SOFT.0)
    }

    pub fn has(self, flag: PolicyFlags) -> bool { self.0 & flag.0 != 0 }
    pub fn set(self, flag: PolicyFlags) -> Self  { PolicyFlags(self.0 | flag.0) }
    pub fn clear(self, flag: PolicyFlags) -> Self { PolicyFlags(self.0 & !flag.0) }
    pub fn is_enabled(self) -> bool { self.has(Self::ENABLED) }
    pub fn is_strict(self)  -> bool { self.has(Self::STRICT) }
    pub fn should_log_soft(self) -> bool { self.has(Self::LOG_SOFT) }
    pub fn should_log_hard(self) -> bool { self.has(Self::LOG_HARD) }
    pub fn should_alert_soft(self) -> bool { self.has(Self::ALERT_SOFT) }
}

// ─── QuotaPolicy ─────────────────────────────────────────────────────────────

/// Politique de quota associant limites, kind et comportement.
#[derive(Clone, Copy, Debug)]
pub struct QuotaPolicy {
    pub kind:   QuotaKind,
    pub limits: QuotaLimits,
    pub flags:  PolicyFlags,
    /// Nom court ASCII[32] de la politique (paddé de zéros).
    pub name:   [u8; 32],
}

impl QuotaPolicy {
    pub const fn default_policy(kind: QuotaKind) -> Self {
        Self {
            kind,
            limits: QuotaLimits::unlimited(),
            flags:  PolicyFlags::default_flags(),
            name:   [0u8; 32],
        }
    }

    pub fn named(kind: QuotaKind, name: &str, limits: QuotaLimits) -> Self {
        let mut p = Self::default_policy(kind);
        p.limits = limits;
        let bytes = name.as_bytes();
        let len = bytes.len().min(32);
        let mut i = 0usize;
        while i < len { p.name[i] = bytes[i]; i = i.wrapping_add(1); }
        p
    }

    pub fn validate(&self) -> ExofsResult<()> {
        self.limits.validate()?;
        Ok(())
    }

    pub fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(32);
        core::str::from_utf8(&self.name[..end]).unwrap_or("<invalid>")
    }

    pub fn is_enabled(&self) -> bool { self.flags.is_enabled() }
    pub fn is_strict(&self)  -> bool { self.flags.is_strict() }

    /// Retourne une politique stricte : soft = hard sur chaque dimension.
    pub fn make_strict(mut self) -> Self {
        self.limits.soft_bytes  = self.limits.hard_bytes;
        self.limits.soft_blobs  = self.limits.hard_blobs;
        self.limits.soft_inodes = self.limits.hard_inodes;
        self.limits.grace_ticks = 0;
        self.flags = self.flags.set(PolicyFlags::STRICT);
        self
    }

    /// Retourne la politique avec de nouvelles limites après validation.
    pub fn with_limits(mut self, lim: QuotaLimits) -> ExofsResult<Self> {
        lim.validate()?;
        self.limits = lim;
        Ok(self)
    }

    /// Vrai si les bytes dépassent la limite dure.
    pub fn bytes_hard_exceeded(&self, used: u64) -> bool {
        self.limits.bytes_hard_exceeded(used)
    }

    /// Vrai si les bytes dépassent la limite souple.
    pub fn bytes_soft_exceeded(&self, used: u64) -> bool {
        self.limits.bytes_soft_exceeded(used)
    }

    /// Vrai si les blobs dépassent la limite dure.
    pub fn blobs_hard_exceeded(&self, used: u64) -> bool {
        self.limits.blobs_hard_exceeded(used)
    }

    /// Vrai si les inodes dépassent la limite dure.
    pub fn inodes_hard_exceeded(&self, used: u64) -> bool {
        self.limits.inodes_hard_exceeded(used)
    }

    /// Score de sévérité (ARITH-02) : max des 3 taux en ‰.
    pub fn severity_score(&self, bytes: u64, blobs: u64, inodes: u64) -> u64 {
        let sb = self.limits.bytes_usage_ppt(bytes);
        let bl = self.limits.blobs_usage_ppt(blobs);
        let si = self.limits.inodes_usage_ppt(inodes);
        sb.max(bl).max(si)
    }
}

// ─── PolicyPresets ────────────────────────────────────────────────────────────

/// Préréglages de politique courants.
pub struct PolicyPresets;

impl PolicyPresets {
    /// Politique utilisateur standard (1 GiB octets, 100k blobs, 50k inodes).
    pub fn standard_user() -> QuotaPolicy {
        QuotaPolicy::named(
            QuotaKind::User,
            "standard_user",
            QuotaLimits {
                soft_bytes:  900 * 1024 * 1024,
                hard_bytes:  1024 * 1024 * 1024,
                soft_blobs:  90_000,
                hard_blobs:  100_000,
                soft_inodes: 45_000,
                hard_inodes: 50_000,
                grace_ticks: 3_600_000_000, // ~1h en µs
            },
        )
    }

    /// Politique projet (10 GiB, 1M blobs, 200k inodes).
    pub fn large_project() -> QuotaPolicy {
        QuotaPolicy::named(
            QuotaKind::Project,
            "large_project",
            QuotaLimits {
                soft_bytes:  9 * 1024 * 1024 * 1024,
                hard_bytes:  10 * 1024 * 1024 * 1024,
                soft_blobs:  900_000,
                hard_blobs:  1_000_000,
                soft_inodes: 180_000,
                hard_inodes: 200_000,
                grace_ticks: 7_200_000_000,
            },
        )
    }

    /// Politique sandbox restrictive (10 MiB, 1000 blobs).
    pub fn sandbox() -> QuotaPolicy {
        QuotaPolicy::named(
            QuotaKind::User,
            "sandbox",
            QuotaLimits {
                soft_bytes:  8 * 1024 * 1024,
                hard_bytes:  10 * 1024 * 1024,
                soft_blobs:  900,
                hard_blobs:  1_000,
                soft_inodes: 900,
                hard_inodes: 1_000,
                grace_ticks: 0,
            },
        ).make_strict()
    }

    /// Politique groupe illimitée.
    pub fn unlimited_group() -> QuotaPolicy {
        QuotaPolicy::default_policy(QuotaKind::Group)
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quota_kind_from_u8() {
        assert_eq!(QuotaKind::from_u8(0), QuotaKind::User);
        assert_eq!(QuotaKind::from_u8(1), QuotaKind::Group);
        assert_eq!(QuotaKind::from_u8(2), QuotaKind::Project);
        assert_eq!(QuotaKind::from_u8(99), QuotaKind::User);
    }

    #[test]
    fn test_quota_limits_unlimited() {
        let l = QuotaLimits::unlimited();
        assert!(l.is_unlimited());
        assert!(l.validate().is_ok());
        assert!(!l.bytes_hard_exceeded(u64::MAX - 1));
    }

    #[test]
    fn test_quota_limits_soft_leq_hard() {
        let l = QuotaLimits {
            soft_bytes: 1000, hard_bytes: 500,
            soft_blobs: 0, hard_blobs: 100,
            soft_inodes: 0, hard_inodes: 100,
            grace_ticks: 0,
        };
        assert!(l.validate().is_err());
    }

    #[test]
    fn test_quota_limits_usage_ppt() {
        let l = QuotaLimits {
            soft_bytes: 800, hard_bytes: 1000,
            soft_blobs: u64::MAX, hard_blobs: u64::MAX,
            soft_inodes: u64::MAX, hard_inodes: u64::MAX,
            grace_ticks: 0,
        };
        assert_eq!(l.bytes_usage_ppt(500), 500);
        assert_eq!(l.bytes_usage_ppt(1000), 1000);
        assert_eq!(l.bytes_usage_ppt(2000), 1000); // capped
        assert_eq!(l.blobs_usage_ppt(9999), 0); // unlimited
    }

    #[test]
    fn test_quota_limits_exceeded() {
        let l = QuotaLimits {
            soft_bytes: 500, hard_bytes: 1000,
            soft_blobs: u64::MAX, hard_blobs: u64::MAX,
            soft_inodes: u64::MAX, hard_inodes: u64::MAX,
            grace_ticks: 0,
        };
        assert!(!l.bytes_hard_exceeded(999));
        assert!(l.bytes_hard_exceeded(1000));
        assert!(l.bytes_soft_exceeded(500));
        assert!(!l.bytes_soft_exceeded(499));
    }

    #[test]
    fn test_quota_limits_remaining() {
        let l = QuotaLimits {
            soft_bytes: 800, hard_bytes: 1000,
            soft_blobs: u64::MAX, hard_blobs: u64::MAX,
            soft_inodes: u64::MAX, hard_inodes: u64::MAX,
            grace_ticks: 0,
        };
        assert_eq!(l.bytes_remaining(600), 400);
        assert_eq!(l.bytes_remaining(1200), 0); // saturating
        assert_eq!(l.blobs_remaining(9999), u64::MAX);
    }

    #[test]
    fn test_policy_flags() {
        let f = PolicyFlags::default_flags();
        assert!(f.is_enabled());
        assert!(f.should_log_hard());
        assert!(f.should_alert_soft());
        assert!(!f.should_log_soft());
        let f2 = f.set(PolicyFlags::LOG_SOFT);
        assert!(f2.should_log_soft());
        let f3 = f2.clear(PolicyFlags::LOG_SOFT);
        assert!(!f3.should_log_soft());
    }

    #[test]
    fn test_policy_default_valid() {
        let p = QuotaPolicy::default_policy(QuotaKind::User);
        assert!(p.validate().is_ok());
        assert!(p.is_enabled());
        assert!(p.limits.is_unlimited());
    }

    #[test]
    fn test_policy_named() {
        let lim = QuotaLimits {
            soft_bytes: 500, hard_bytes: 1000,
            soft_blobs: u64::MAX, hard_blobs: u64::MAX,
            soft_inodes: u64::MAX, hard_inodes: u64::MAX,
            grace_ticks: 0,
        };
        let p = QuotaPolicy::named(QuotaKind::User, "test", lim);
        assert_eq!(p.name_str(), "test");
        assert!(p.validate().is_ok());
    }

    #[test]
    fn test_policy_make_strict() {
        let lim = QuotaLimits {
            soft_bytes: 500, hard_bytes: 1000,
            soft_blobs: u64::MAX, hard_blobs: u64::MAX,
            soft_inodes: u64::MAX, hard_inodes: u64::MAX,
            grace_ticks: 100,
        };
        let p = QuotaPolicy::named(QuotaKind::User, "strict", lim).make_strict();
        assert!(p.is_strict());
        assert_eq!(p.limits.soft_bytes, 1000);
        assert_eq!(p.limits.grace_ticks, 0);
    }

    #[test]
    fn test_policy_severity_score() {
        let p = PolicyPresets::standard_user();
        let score = p.severity_score(
            950 * 1024 * 1024,
            10_000,
            5_000,
        );
        assert!(score > 0 && score <= 1000);
    }

    #[test]
    fn test_presets_validate() {
        assert!(PolicyPresets::standard_user().validate().is_ok());
        assert!(PolicyPresets::large_project().validate().is_ok());
        assert!(PolicyPresets::sandbox().validate().is_ok());
        assert!(PolicyPresets::unlimited_group().validate().is_ok());
    }

    #[test]
    fn test_policy_with_limits() {
        let p = QuotaPolicy::default_policy(QuotaKind::Group);
        let lim = QuotaLimits {
            soft_bytes: 100, hard_bytes: 200,
            soft_blobs: u64::MAX, hard_blobs: u64::MAX,
            soft_inodes: u64::MAX, hard_inodes: u64::MAX,
            grace_ticks: 0,
        };
        let p2 = p.with_limits(lim).expect("ok");
        assert_eq!(p2.limits.hard_bytes, 200);
    }

    #[test]
    fn test_policy_with_limits_invalid() {
        let p = QuotaPolicy::default_policy(QuotaKind::Group);
        let bad = QuotaLimits {
            soft_bytes: 5000, hard_bytes: 100,
            soft_blobs: u64::MAX, hard_blobs: u64::MAX,
            soft_inodes: u64::MAX, hard_inodes: u64::MAX,
            grace_ticks: 0,
        };
        assert!(p.with_limits(bad).is_err());
    }
}
