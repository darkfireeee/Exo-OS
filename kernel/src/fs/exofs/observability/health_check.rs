//! health_check.rs — Vérification de l'état de santé du filesystem ExoFS (no_std).

use core::sync::atomic::{AtomicU8, Ordering};
use super::space_tracker::SPACE_TRACKER;
use super::metrics::EXOFS_METRICS;

pub static HEALTH: HealthCheck = HealthCheck::new_const();

/// État de santé du filesystem.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealthStatus {
    Healthy     = 0,
    Degraded    = 1,   // Performances dégradées mais fonctionnel.
    Critical    = 2,   // < 5% espace libre, erreurs fréquentes.
    ReadOnly    = 3,   // Monté en lecture seule suite à des erreurs.
    Unmounted   = 4,
}

pub struct HealthCheck {
    status: AtomicU8,
}

impl HealthCheck {
    pub const fn new_const() -> Self {
        Self { status: AtomicU8::new(HealthStatus::Healthy as u8) }
    }

    pub fn status(&self) -> HealthStatus {
        match self.status.load(Ordering::Relaxed) {
            1 => HealthStatus::Degraded,
            2 => HealthStatus::Critical,
            3 => HealthStatus::ReadOnly,
            4 => HealthStatus::Unmounted,
            _ => HealthStatus::Healthy,
        }
    }

    pub fn set_status(&self, s: HealthStatus) {
        self.status.store(s as u8, Ordering::Relaxed);
    }

    /// Réévalue et met à jour le statut de santé global.
    pub fn evaluate(&self) {
        let usage_pct  = SPACE_TRACKER.usage_pct();
        let errors     = EXOFS_METRICS.errors.load(Ordering::Relaxed);

        let new_status = if usage_pct > 95 || errors > 1000 {
            HealthStatus::Critical
        } else if usage_pct > 85 || errors > 100 {
            HealthStatus::Degraded
        } else {
            HealthStatus::Healthy
        };

        // Ne jamais rétrograder ReadOnly automatiquement.
        let cur = self.status();
        if cur != HealthStatus::ReadOnly && cur != HealthStatus::Unmounted {
            self.set_status(new_status);
        }
    }

    pub fn is_writable(&self) -> bool {
        matches!(self.status(), HealthStatus::Healthy | HealthStatus::Degraded)
    }
}
