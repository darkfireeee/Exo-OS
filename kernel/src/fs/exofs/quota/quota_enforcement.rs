//! QuotaEnforcement — application des limites de quota ExoFS (no_std).

use crate::fs::exofs::core::FsError;
use super::quota_tracker::{QUOTA_TRACKER, QuotaKey};
use super::quota_policy::{QuotaPolicy, QuotaLimits};
use super::quota_audit::QUOTA_AUDIT;

/// Résultat d'un contrôle de quota.
#[derive(Debug, Clone, Copy)]
pub enum EnforcementResult {
    Allowed,
    SoftBreach { entity_id: u64, current_bytes: u64, soft_limit: u64 },
    HardDenied { entity_id: u64, current_bytes: u64, hard_limit: u64 },
}

/// Gestionnaire d'application des quotas.
pub struct QuotaEnforcement {
    policy: QuotaPolicy,
}

impl QuotaEnforcement {
    pub fn new(policy: QuotaPolicy) -> Self {
        Self { policy }
    }

    /// Vérifie si une opération d'écriture est autorisée.
    pub fn check_write(
        &self,
        key: QuotaKey,
        request_bytes: u64,
    ) -> EnforcementResult {
        if !self.policy.enabled {
            return EnforcementResult::Allowed;
        }

        let limits = match key.kind {
            0 => &self.policy.user_limits,
            1 => &self.policy.group_limits,
            _ => &self.policy.project_limits,
        };

        let usage = QUOTA_TRACKER.get_usage(&key);
        let new_bytes = usage.bytes_used.saturating_add(request_bytes);

        if new_bytes > limits.hard_bytes {
            if self.policy.enforce_hard {
                QUOTA_AUDIT.record_hard_denial(key.entity_id, new_bytes, limits.hard_bytes);
                return EnforcementResult::HardDenied {
                    entity_id:     key.entity_id,
                    current_bytes: new_bytes,
                    hard_limit:    limits.hard_bytes,
                };
            }
        }

        if new_bytes > limits.soft_bytes && self.policy.log_soft_breach {
            QUOTA_AUDIT.record_soft_breach(key.entity_id, new_bytes, limits.soft_bytes);
            return EnforcementResult::SoftBreach {
                entity_id:    key.entity_id,
                current_bytes: new_bytes,
                soft_limit:   limits.soft_bytes,
            };
        }

        EnforcementResult::Allowed
    }

    /// Effective charge après vérification.
    pub fn charge_and_check(
        &self,
        key: QuotaKey,
        bytes: u64,
        blobs: u64,
        inodes: u64,
    ) -> Result<EnforcementResult, FsError> {
        let result = self.check_write(key, bytes);
        if let EnforcementResult::HardDenied { .. } = result {
            return Ok(result);
        }
        QUOTA_TRACKER.charge(key, bytes, blobs, inodes)?;
        Ok(result)
    }
}
