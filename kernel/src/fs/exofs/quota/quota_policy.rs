// SPDX-License-Identifier: GPL-3.0
// ExoFS Quota — Policy and Limits
// ≥400L, ExofsError only, RECUR-01/OOM-02/ARITH-02

use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─── QuotaKind ────────────────────────────────────────────────────────────────

/// Type of entity subject to a quota.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum QuotaKind {
    User = 0,
    Group = 1,
    Project = 2,
}

impl QuotaKind {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Group,
            2 => Self::Project,
            _ => Self::User,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Group => "group",
            Self::Project => "project",
        }
    }
    pub const COUNT: usize = 3;
}

// ─── QuotaLimits ─────────────────────────────────────────────────────────────

/// Quota limits for an entity (bytes, blobs, inodes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuotaLimits {
    /// Soft limit in bytes (warning).
    pub soft_bytes: u64,
    /// Hard limit in bytes (immediate denial).
    pub hard_bytes: u64,
    /// Soft limit in number of blobs.
    pub soft_blobs: u64,
    /// Hard limit in number of blobs.
    pub hard_blobs: u64,
    /// Soft limit in number of inodes.
    pub soft_inodes: u64,
    /// Hard limit in number of inodes.
    pub hard_inodes: u64,
    /// Grace period duration after soft breach (in ticks).
    pub grace_ticks: u64,
}

impl QuotaLimits {
    /// Unlimited quotas (default value).
    pub const fn unlimited() -> Self {
        Self {
            soft_bytes: u64::MAX,
            hard_bytes: u64::MAX,
            soft_blobs: u64::MAX,
            hard_blobs: u64::MAX,
            soft_inodes: u64::MAX,
            hard_inodes: u64::MAX,
            grace_ticks: 0,
        }
    }

    /// Validates limits: soft <= hard for each dimension.
    pub fn validate(&self) -> ExofsResult<()> {
        if !Self::pair_is_valid(self.soft_bytes, self.hard_bytes) {
            return Err(ExofsError::InvalidArgument);
        }
        if !Self::pair_is_valid(self.soft_blobs, self.hard_blobs) {
            return Err(ExofsError::InvalidArgument);
        }
        if !Self::pair_is_valid(self.soft_inodes, self.hard_inodes) {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(())
    }

    const fn pair_is_valid(soft: u64, hard: u64) -> bool {
        soft == u64::MAX || hard == u64::MAX || soft <= hard
    }

    /// True if all limits are at MAX (non-constraining).
    pub fn is_unlimited(&self) -> bool {
        self.hard_bytes == u64::MAX && self.hard_blobs == u64::MAX && self.hard_inodes == u64::MAX
    }

    /// Calculation of byte usage percentage in ‰ (ARITH-02).
    pub fn bytes_usage_ppt(&self, used: u64) -> u64 {
        if self.hard_bytes == 0 || self.hard_bytes == u64::MAX {
            return 0;
        }
        used.saturating_mul(1000)
            .checked_div(self.hard_bytes)
            .unwrap_or(1000)
            .min(1000)
    }

    /// Calculation of blob usage percentage in ‰.
    pub fn blobs_usage_ppt(&self, used: u64) -> u64 {
        if self.hard_blobs == 0 || self.hard_blobs == u64::MAX {
            return 0;
        }
        used.saturating_mul(1000)
            .checked_div(self.hard_blobs)
            .unwrap_or(1000)
            .min(1000)
    }

    /// Calculation of inode usage percentage in ‰.
    pub fn inodes_usage_ppt(&self, used: u64) -> u64 {
        if self.hard_inodes == 0 || self.hard_inodes == u64::MAX {
            return 0;
        }
        used.saturating_mul(1000)
            .checked_div(self.hard_inodes)
            .unwrap_or(1000)
            .min(1000)
    }

    /// True if hard byte limit is exceeded.
    pub fn bytes_hard_exceeded(&self, used: u64) -> bool {
        self.hard_bytes != u64::MAX && used >= self.hard_bytes
    }

    /// True if soft byte limit is exceeded.
    pub fn bytes_soft_exceeded(&self, used: u64) -> bool {
        self.soft_bytes != u64::MAX && used >= self.soft_bytes
    }

    /// True if hard blob limit is exceeded.
    pub fn blobs_hard_exceeded(&self, used: u64) -> bool {
        self.hard_blobs != u64::MAX && used >= self.hard_blobs
    }

    /// True if soft blob limit is exceeded.
    pub fn blobs_soft_exceeded(&self, used: u64) -> bool {
        self.soft_blobs != u64::MAX && used >= self.soft_blobs
    }

    /// True if hard inode limit is exceeded.
    pub fn inodes_hard_exceeded(&self, used: u64) -> bool {
        self.hard_inodes != u64::MAX && used >= self.hard_inodes
    }

    /// True if soft inode limit is exceeded.
    pub fn inodes_soft_exceeded(&self, used: u64) -> bool {
        self.soft_inodes != u64::MAX && used >= self.soft_inodes
    }

    /// Bytes available before hard limit (saturating).
    pub fn bytes_remaining(&self, used: u64) -> u64 {
        if self.hard_bytes == u64::MAX {
            return u64::MAX;
        }
        self.hard_bytes.saturating_sub(used)
    }

    /// Blobs available before hard limit.
    pub fn blobs_remaining(&self, used: u64) -> u64 {
        if self.hard_blobs == u64::MAX {
            return u64::MAX;
        }
        self.hard_blobs.saturating_sub(used)
    }

    /// Inodes available before hard limit.
    pub fn inodes_remaining(&self, used: u64) -> u64 {
        if self.hard_inodes == u64::MAX {
            return u64::MAX;
        }
        self.hard_inodes.saturating_sub(used)
    }
}

// ─── PolicyFlags ─────────────────────────────────────────────────────────────

/// Behavior flags of a quota policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PolicyFlags(pub u32);

impl PolicyFlags {
    /// Policy enabled.
    pub const ENABLED: PolicyFlags = PolicyFlags(1 << 0);
    /// Log soft breaches.
    pub const LOG_SOFT: PolicyFlags = PolicyFlags(1 << 1);
    /// Log hard denials.
    pub const LOG_HARD: PolicyFlags = PolicyFlags(1 << 2);
    /// Send an alert upon soft breach.
    pub const ALERT_SOFT: PolicyFlags = PolicyFlags(1 << 3);
    /// Inheritance of limits from parent namespace.
    pub const INHERIT: PolicyFlags = PolicyFlags(1 << 4);
    /// Strict mode: soft = hard (no grace period).
    pub const STRICT: PolicyFlags = PolicyFlags(1 << 5);
    /// Enable logging of limit modifications.
    pub const AUDIT_LIMITS: PolicyFlags = PolicyFlags(1 << 6);

    pub const fn default_flags() -> Self {
        // ENABLED | LOG_HARD | ALERT_SOFT
        PolicyFlags(Self::ENABLED.0 | Self::LOG_HARD.0 | Self::ALERT_SOFT.0)
    }

    pub fn has(self, flag: PolicyFlags) -> bool {
        self.0 & flag.0 != 0
    }
    pub fn set(self, flag: PolicyFlags) -> Self {
        PolicyFlags(self.0 | flag.0)
    }
    pub fn clear(self, flag: PolicyFlags) -> Self {
        PolicyFlags(self.0 & !flag.0)
    }
    pub fn is_enabled(self) -> bool {
        self.has(Self::ENABLED)
    }
    pub fn is_strict(self) -> bool {
        self.has(Self::STRICT)
    }
    pub fn should_log_soft(self) -> bool {
        self.has(Self::LOG_SOFT)
    }
    pub fn should_log_hard(self) -> bool {
        self.has(Self::LOG_HARD)
    }
    pub fn should_alert_soft(self) -> bool {
        self.has(Self::ALERT_SOFT)
    }
}

// ─── QuotaPolicy ─────────────────────────────────────────────────────────────

/// Quota policy associating limits, kind, and behavior.
#[derive(Clone, Copy, Debug)]
pub struct QuotaPolicy {
    pub kind: QuotaKind,
    pub limits: QuotaLimits,
    pub flags: PolicyFlags,
    /// ASCII[32] short name of the policy (zero-padded).
    pub name: [u8; 32],
}

impl QuotaPolicy {
    pub const fn default_policy(kind: QuotaKind) -> Self {
        Self {
            kind,
            limits: QuotaLimits::unlimited(),
            flags: PolicyFlags::default_flags(),
            name: [0u8; 32],
        }
    }

    pub fn named(kind: QuotaKind, name: &str, limits: QuotaLimits) -> Self {
        let mut p = Self::default_policy(kind);
        p.limits = limits;
        let bytes = name.as_bytes();
        let len = bytes.len().min(32);
        let mut i = 0usize;
        while i < len {
            p.name[i] = bytes[i];
            i = i.wrapping_add(1);
        }
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

    pub fn is_enabled(&self) -> bool {
        self.flags.is_enabled()
    }
    pub fn is_strict(&self) -> bool {
        self.flags.is_strict()
    }

    /// Returns a strict policy: soft = hard on each dimension.
    pub fn make_strict(mut self) -> Self {
        self.limits.soft_bytes = self.limits.hard_bytes;
        self.limits.soft_blobs = self.limits.hard_blobs;
        self.limits.soft_inodes = self.limits.hard_inodes;
        self.limits.grace_ticks = 0;
        self.flags = self.flags.set(PolicyFlags::STRICT);
        self
    }

    /// Returns the policy with new limits after validation.
    pub fn with_limits(mut self, lim: QuotaLimits) -> ExofsResult<Self> {
        lim.validate()?;
        self.limits = lim;
        Ok(self)
    }

    /// True if bytes exceed hard limit.
    pub fn bytes_hard_exceeded(&self, used: u64) -> bool {
        self.limits.bytes_hard_exceeded(used)
    }

    /// True if bytes exceed soft limit.
    pub fn bytes_soft_exceeded(&self, used: u64) -> bool {
        self.limits.bytes_soft_exceeded(used)
    }

    /// True if blobs exceed hard limit.
    pub fn blobs_hard_exceeded(&self, used: u64) -> bool {
        self.limits.blobs_hard_exceeded(used)
    }

    /// True if inodes exceed hard limit.
    pub fn inodes_hard_exceeded(&self, used: u64) -> bool {
        self.limits.inodes_hard_exceeded(used)
    }

    /// Severity score (ARITH-02): max of the 3 ratios in ‰.
    pub fn severity_score(&self, bytes: u64, blobs: u64, inodes: u64) -> u64 {
        let sb = self.limits.bytes_usage_ppt(bytes);
        let bl = self.limits.blobs_usage_ppt(blobs);
        let si = self.limits.inodes_usage_ppt(inodes);
        sb.max(bl).max(si)
    }
}

// ─── PolicyPresets ────────────────────────────────────────────────────────────

/// Common policy presets.
pub struct PolicyPresets;

impl PolicyPresets {
    /// Standard user policy (1 GiB bytes, 100k blobs, 50k inodes).
    pub fn standard_user() -> QuotaPolicy {
        QuotaPolicy::named(
            QuotaKind::User,
            "standard_user",
            QuotaLimits {
                soft_bytes: 900 * 1024 * 1024,
                hard_bytes: 1024 * 1024 * 1024,
                soft_blobs: 90_000,
                hard_blobs: 100_000,
                soft_inodes: 45_000,
                hard_inodes: 50_000,
                grace_ticks: 3_600_000_000, // ~1h in µs
            },
        )
    }

    /// Project policy (10 GiB, 1M blobs, 200k inodes).
    pub fn large_project() -> QuotaPolicy {
        QuotaPolicy::named(
            QuotaKind::Project,
            "large_project",
            QuotaLimits {
                soft_bytes: 9 * 1024 * 1024 * 1024,
                hard_bytes: 10 * 1024 * 1024 * 1024,
                soft_blobs: 900_000,
                hard_blobs: 1_000_000,
                soft_inodes: 180_000,
                hard_inodes: 200_000,
                grace_ticks: 7_200_000_000,
            },
        )
    }

    /// Restrictive sandbox policy (10 MiB, 1000 blobs).
    pub fn sandbox() -> QuotaPolicy {
        QuotaPolicy::named(
            QuotaKind::User,
            "sandbox",
            QuotaLimits {
                soft_bytes: 8 * 1024 * 1024,
                hard_bytes: 10 * 1024 * 1024,
                soft_blobs: 900,
                hard_blobs: 1_000,
                soft_inodes: 900,
                hard_inodes: 1_000,
                grace_ticks: 0,
            },
        )
        .make_strict()
    }

    /// Unlimited group policy.
    pub fn unlimited_group() -> QuotaPolicy {
        QuotaPolicy::default_policy(QuotaKind::Group)
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
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
            soft_bytes: 1000,
            hard_bytes: 500,
            soft_blobs: 0,
            hard_blobs: 100,
            soft_inodes: 0,
            hard_inodes: 100,
            grace_ticks: 0,
        };
        assert!(l.validate().is_err());
    }

    #[test]
    fn test_quota_limits_usage_ppt() {
        let l = QuotaLimits {
            soft_bytes: 800,
            hard_bytes: 1000,
            soft_blobs: u64::MAX,
            hard_blobs: u64::MAX,
            soft_inodes: u64::MAX,
            hard_inodes: u64::MAX,
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
            soft_bytes: 500,
            hard_bytes: 1000,
            soft_blobs: u64::MAX,
            hard_blobs: u64::MAX,
            soft_inodes: u64::MAX,
            hard_inodes: u64::MAX,
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
            soft_bytes: 800,
            hard_bytes: 1000,
            soft_blobs: u64::MAX,
            hard_blobs: u64::MAX,
            soft_inodes: u64::MAX,
            hard_inodes: u64::MAX,
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
            soft_bytes: 500,
            hard_bytes: 1000,
            soft_blobs: u64::MAX,
            hard_blobs: u64::MAX,
            soft_inodes: u64::MAX,
            hard_inodes: u64::MAX,
            grace_ticks: 0,
        };
        let p = QuotaPolicy::named(QuotaKind::User, "test", lim);
        assert_eq!(p.name_str(), "test");
        assert!(p.validate().is_ok());
    }

    #[test]
    fn test_policy_make_strict() {
        let lim = QuotaLimits {
            soft_bytes: 500,
            hard_bytes: 1000,
            soft_blobs: u64::MAX,
            hard_blobs: u64::MAX,
            soft_inodes: u64::MAX,
            hard_inodes: u64::MAX,
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
        let score = p.severity_score(950 * 1024 * 1024, 10_000, 5_000);
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
            soft_bytes: 100,
            hard_bytes: 200,
            soft_blobs: u64::MAX,
            hard_blobs: u64::MAX,
            soft_inodes: u64::MAX,
            hard_inodes: u64::MAX,
            grace_ticks: 0,
        };
        let p2 = p.with_limits(lim).test_expect("ok");
        assert_eq!(p2.limits.hard_bytes, 200);
    }

    #[test]
    fn test_policy_with_limits_invalid() {
        let p = QuotaPolicy::default_policy(QuotaKind::Group);
        let bad = QuotaLimits {
            soft_bytes: 5000,
            hard_bytes: 100,
            soft_blobs: u64::MAX,
            hard_blobs: u64::MAX,
            soft_inodes: u64::MAX,
            hard_inodes: u64::MAX,
            grace_ticks: 0,
        };
        assert!(p.with_limits(bad).is_err());
    }
}
