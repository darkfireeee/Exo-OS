//! AuditRotation — politique de rotation du journal d'audit ExoFS (no_std).

/// Configuration de rotation.
#[derive(Clone, Debug)]
pub struct RotationConfig {
    pub max_entries:   u64,
    pub max_age_ticks: u64,
}

impl Default for RotationConfig {
    fn default() -> Self {
        Self {
            max_entries:   1_000_000,
            max_age_ticks: 30 * 24 * 3600 * 1_000_000_000u64,
        }
    }
}

pub struct AuditRotation {
    pub config: RotationConfig,
    rotations:  core::sync::atomic::AtomicU64,
}

impl AuditRotation {
    pub const fn new() -> Self {
        Self {
            config:    RotationConfig { max_entries: 1_000_000, max_age_ticks: 0 },
            rotations: core::sync::atomic::AtomicU64::new(0),
        }
    }

    pub fn check_and_rotate(&self) {
        let count = crate::fs::exofs::audit::audit_log::AUDIT_LOG.count();
        if count > self.config.max_entries {
            self.rotations.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }
    }

    pub fn rotation_count(&self) -> u64 {
        self.rotations.load(core::sync::atomic::Ordering::Relaxed)
    }
}
