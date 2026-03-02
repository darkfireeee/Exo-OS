//! QuotaPolicy et QuotaLimits — politiques et limites de quota ExoFS (no_std).

/// Type de quota.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuotaKind {
    User    = 0,
    Group   = 1,
    Project = 2,
}

/// Limites de quota pour une entité.
#[derive(Clone, Copy, Debug)]
pub struct QuotaLimits {
    pub soft_bytes:  u64,   // Limite souple (avertissement).
    pub hard_bytes:  u64,   // Limite dure (blocage).
    pub soft_blobs:  u64,
    pub hard_blobs:  u64,
    pub soft_inodes: u64,
    pub hard_inodes: u64,
    pub grace_ticks: u64,   // Grâce avant blocage après dépassement soft.
}

impl QuotaLimits {
    pub fn unlimited() -> Self {
        Self {
            soft_bytes: u64::MAX, hard_bytes: u64::MAX,
            soft_blobs: u64::MAX, hard_blobs: u64::MAX,
            soft_inodes: u64::MAX, hard_inodes: u64::MAX,
            grace_ticks: 0,
        }
    }

    pub fn new_1gib() -> Self {
        Self {
            soft_bytes:  900 * 1024 * 1024,
            hard_bytes:  1024 * 1024 * 1024,
            soft_blobs:  90_000,
            hard_blobs:  100_000,
            soft_inodes: 90_000,
            hard_inodes: 100_000,
            grace_ticks: 7 * 24 * 3600 * 1_000_000_000,
        }
    }
}

/// Politique globale des quotas.
#[derive(Clone, Debug)]
pub struct QuotaPolicy {
    pub enabled:          bool,
    pub user_limits:      QuotaLimits,
    pub group_limits:     QuotaLimits,
    pub project_limits:   QuotaLimits,
    pub enforce_hard:     bool,
    pub log_soft_breach:  bool,
}

impl QuotaPolicy {
    pub fn default_enabled() -> Self {
        Self {
            enabled:         true,
            user_limits:     QuotaLimits::new_1gib(),
            group_limits:    QuotaLimits::new_1gib(),
            project_limits:  QuotaLimits::unlimited(),
            enforce_hard:    true,
            log_soft_breach: true,
        }
    }

    pub fn disabled() -> Self {
        Self {
            enabled:         false,
            user_limits:     QuotaLimits::unlimited(),
            group_limits:    QuotaLimits::unlimited(),
            project_limits:  QuotaLimits::unlimited(),
            enforce_hard:    false,
            log_soft_breach: false,
        }
    }
}
