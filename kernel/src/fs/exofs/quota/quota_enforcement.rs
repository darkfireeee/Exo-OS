// SPDX-License-Identifier: MIT
// ExoFS Quota — Enforcement (application des limites)
// ≥400L, ExofsError only, RECUR-01/OOM-02/ARITH-02

use super::quota_audit::{QuotaEvent, QUOTA_AUDIT};
use super::quota_tracker::{QuotaKey, QuotaUsage, QUOTA_TRACKER};
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;

// ─── EnforcementAction ────────────────────────────────────────────────────────

/// Action décidée après vérification d'une limite.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnforcementAction {
    /// Opération autorisée sans réserve.
    Allow,
    /// Opération autorisée, mais seuil soft franchi.
    AllowWithWarning,
    /// Opération refusée : limite hard dépassée.
    Deny,
    /// Opération refusée : délai de grâce expiré.
    DenyGrace,
}

impl EnforcementAction {
    pub fn is_deny(self) -> bool {
        matches!(self, Self::Deny | Self::DenyGrace)
    }
    pub fn is_allow(self) -> bool {
        !self.is_deny()
    }
    pub fn reason_str(self) -> &'static str {
        match self {
            Self::Allow => "allowed",
            Self::AllowWithWarning => "allowed-soft-breach",
            Self::Deny => "denied-hard",
            Self::DenyGrace => "denied-grace-expired",
        }
    }
}

// ─── EnforcementResult ────────────────────────────────────────────────────────

/// Résultat détaillé d'une vérification de quota.
#[derive(Clone, Copy, Debug)]
pub struct EnforcementResult {
    pub action: EnforcementAction,
    pub entity_id: u64,
    pub used_bytes: u64,
    pub used_blobs: u64,
    pub used_inodes: u64,
    pub limit_bytes: u64,
    pub limit_blobs: u64,
    pub limit_inodes: u64,
    pub grace_ticks: u64,
    /// Taille de la requête en octets.
    pub requested: u64,
}

impl EnforcementResult {
    pub fn allow(key: QuotaKey) -> Self {
        Self {
            action: EnforcementAction::Allow,
            entity_id: key.entity_id,
            used_bytes: 0,
            used_blobs: 0,
            used_inodes: 0,
            limit_bytes: 0,
            limit_blobs: 0,
            limit_inodes: 0,
            grace_ticks: 0,
            requested: 0,
        }
    }

    pub fn is_allowed(self) -> bool {
        self.action.is_allow()
    }
    pub fn is_denied(self) -> bool {
        self.action.is_deny()
    }

    /// Fraction utilisée en ‰ (bytes), ARITH-02.
    pub fn bytes_ppt(self) -> u64 {
        if self.limit_bytes == 0 || self.limit_bytes == u64::MAX {
            return 0;
        }
        self.used_bytes
            .saturating_mul(1000)
            .checked_div(self.limit_bytes)
            .unwrap_or(1000)
    }

    /// Fraction utilisée en ‰ (inodes), ARITH-02.
    pub fn inodes_ppt(self) -> u64 {
        if self.limit_inodes == 0 || self.limit_inodes == u64::MAX {
            return 0;
        }
        self.used_inodes
            .saturating_mul(1000)
            .checked_div(self.limit_inodes)
            .unwrap_or(1000)
    }

    pub fn reason_str(self) -> &'static str {
        self.action.reason_str()
    }
}

// ─── QuotaViolation ───────────────────────────────────────────────────────────

/// Violation de quota enregistrée pour l'audit.
#[derive(Clone, Copy, Debug)]
pub struct QuotaViolation {
    pub key: QuotaKey,
    pub event: QuotaEvent,
    pub bytes_requested: u64,
    pub current_bytes: u64,
    pub limit_bytes: u64,
    pub tick: u64,
}

impl QuotaViolation {
    pub fn new(
        key: QuotaKey,
        event: QuotaEvent,
        bytes_requested: u64,
        current_bytes: u64,
        limit_bytes: u64,
        tick: u64,
    ) -> Self {
        Self {
            key,
            event,
            bytes_requested,
            current_bytes,
            limit_bytes,
            tick,
        }
    }

    pub fn overshoot_bytes(&self) -> u64 {
        let total = self.current_bytes.saturating_add(self.bytes_requested);
        total.saturating_sub(self.limit_bytes)
    }
}

// ─── EnforcementStats ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Default)]
pub struct EnforcementStats {
    pub checks_total: u64,
    pub allowed: u64,
    pub soft_breaches: u64,
    pub hard_denials: u64,
    pub grace_denials: u64,
}

impl EnforcementStats {
    pub const fn zero() -> Self {
        Self {
            checks_total: 0,
            allowed: 0,
            soft_breaches: 0,
            hard_denials: 0,
            grace_denials: 0,
        }
    }
    pub fn denial_rate_ppt(&self) -> u64 {
        if self.checks_total == 0 {
            return 0;
        }
        let denials = self.hard_denials.saturating_add(self.grace_denials);
        denials
            .saturating_mul(1000)
            .checked_div(self.checks_total)
            .unwrap_or(0)
    }
    pub fn is_healthy(&self) -> bool {
        self.denial_rate_ppt() < 50
    }
}

// ─── QuotaEnforcer ────────────────────────────────────────────────────────────

/// Moteur d'application des limites de quota.
pub struct QuotaEnforcer {
    stats: core::cell::UnsafeCell<EnforcementStats>,
    lock: core::sync::atomic::AtomicU64,
}

unsafe impl Sync for QuotaEnforcer {}
unsafe impl Send for QuotaEnforcer {}

impl QuotaEnforcer {
    pub const fn new_const() -> Self {
        Self {
            stats: core::cell::UnsafeCell::new(EnforcementStats::zero()),
            lock: core::sync::atomic::AtomicU64::new(0),
        }
    }

    fn acquire(&self) {
        use core::sync::atomic::Ordering;
        while self
            .lock
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
    }
    fn release(&self) {
        use core::sync::atomic::Ordering;
        self.lock.store(0, Ordering::Release);
    }

    fn add_check(&self, result: &EnforcementResult) {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let s = unsafe { &mut *self.stats.get() };
        s.checks_total = s.checks_total.saturating_add(1);
        match result.action {
            EnforcementAction::Allow => s.allowed = s.allowed.saturating_add(1),
            EnforcementAction::AllowWithWarning => {
                s.allowed = s.allowed.saturating_add(1);
                s.soft_breaches = s.soft_breaches.saturating_add(1);
            }
            EnforcementAction::Deny => s.hard_denials = s.hard_denials.saturating_add(1),
            EnforcementAction::DenyGrace => s.grace_denials = s.grace_denials.saturating_add(1),
        }
        self.release();
    }

    pub fn stats(&self) -> EnforcementStats {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let s = unsafe { *self.stats.get() };
        self.release();
        s
    }

    // ── Vérifications ────────────────────────────────────────────────────────

    /// Vérifie si l'entité `key` peut consommer `request_bytes` octets supplémentaires.
    pub fn check_bytes(
        &self,
        key: QuotaKey,
        request_bytes: u64,
        tick: u64,
    ) -> ExofsResult<EnforcementResult> {
        let entry = QUOTA_TRACKER.get_entry(key);
        // Si aucune entrée, quota non configuré → autorisé
        let limits = match QUOTA_TRACKER.get_limits(key) {
            Some(l) if !l.is_unlimited() => l,
            _ => return Ok(self._make_allow(key)),
        };
        let used = QUOTA_TRACKER
            .get_usage(key)
            .map(|u| u.bytes_used)
            .unwrap_or(0);
        let projected = used.saturating_add(request_bytes);

        let result = if limits.hard_bytes != u64::MAX && projected > limits.hard_bytes {
            // Vérifier le délai de grâce
            let grace_ok = if limits.grace_ticks > 0 {
                entry.map(|e| !e.grace_expired(tick)).unwrap_or(false)
            } else {
                false
            };

            if grace_ok {
                let _remaining_grace = entry
                    .map(|e| {
                        e.soft_breach_tick
                            .saturating_add(limits.grace_ticks)
                            .saturating_sub(tick)
                    })
                    .unwrap_or(0);
                self._build(
                    key,
                    EnforcementAction::DenyGrace,
                    used,
                    limits.hard_bytes,
                    request_bytes,
                    tick,
                )
            } else {
                QUOTA_AUDIT.log_hard_denial(key, used, limits.hard_bytes);
                self._build(
                    key,
                    EnforcementAction::Deny,
                    used,
                    limits.hard_bytes,
                    request_bytes,
                    tick,
                )
            }
        } else if limits.soft_bytes != u64::MAX && projected > limits.soft_bytes {
            // Soft breach
            if let Some(en) = entry {
                if en.soft_breach_tick == 0 {
                    QUOTA_TRACKER.record_soft_breach(key, tick);
                }
            }
            QUOTA_AUDIT.log_soft_breach(key, used, limits.soft_bytes);
            self._build(
                key,
                EnforcementAction::AllowWithWarning,
                used,
                limits.soft_bytes,
                request_bytes,
                tick,
            )
        } else {
            self._make_allow(key)
        };

        self.add_check(&result);
        Ok(result)
    }

    /// Vérifie les blobs.
    pub fn check_blobs(
        &self,
        key: QuotaKey,
        request_blobs: u64,
        tick: u64,
    ) -> ExofsResult<EnforcementResult> {
        let limits = match QUOTA_TRACKER.get_limits(key) {
            Some(l) if !l.is_unlimited() => l,
            _ => return Ok(self._make_allow(key)),
        };
        let used = QUOTA_TRACKER
            .get_usage(key)
            .map(|u| u.blobs_used)
            .unwrap_or(0);
        let projected = used.saturating_add(request_blobs);

        let result = if limits.hard_blobs != u64::MAX && projected > limits.hard_blobs {
            QUOTA_AUDIT.log_hard_denial(key, used, limits.hard_blobs);
            self._build(
                key,
                EnforcementAction::Deny,
                used,
                limits.hard_blobs,
                request_blobs,
                tick,
            )
        } else if limits.soft_blobs != u64::MAX && projected > limits.soft_blobs {
            QUOTA_AUDIT.log_soft_breach(key, used, limits.soft_blobs);
            self._build(
                key,
                EnforcementAction::AllowWithWarning,
                used,
                limits.soft_blobs,
                request_blobs,
                tick,
            )
        } else {
            self._make_allow(key)
        };

        self.add_check(&result);
        Ok(result)
    }

    /// Vérifie les inodes.
    pub fn check_inodes(
        &self,
        key: QuotaKey,
        request_inodes: u64,
        tick: u64,
    ) -> ExofsResult<EnforcementResult> {
        let limits = match QUOTA_TRACKER.get_limits(key) {
            Some(l) if !l.is_unlimited() => l,
            _ => return Ok(self._make_allow(key)),
        };
        let used = QUOTA_TRACKER
            .get_usage(key)
            .map(|u| u.inodes_used)
            .unwrap_or(0);
        let projected = used.saturating_add(request_inodes);

        let result = if limits.hard_inodes != u64::MAX && projected > limits.hard_inodes {
            QUOTA_AUDIT.log_hard_denial(key, used, limits.hard_inodes);
            self._build(
                key,
                EnforcementAction::Deny,
                used,
                limits.hard_inodes,
                request_inodes,
                tick,
            )
        } else if limits.soft_inodes != u64::MAX && projected > limits.soft_inodes {
            QUOTA_AUDIT.log_soft_breach(key, used, limits.soft_inodes);
            self._build(
                key,
                EnforcementAction::AllowWithWarning,
                used,
                limits.soft_inodes,
                request_inodes,
                tick,
            )
        } else {
            self._make_allow(key)
        };

        self.add_check(&result);
        Ok(result)
    }

    /// Vérification combinée bytes + blobs + inodes.
    pub fn check_write(
        &self,
        key: QuotaKey,
        bytes: u64,
        blobs: u64,
        inodes: u64,
        tick: u64,
    ) -> ExofsResult<EnforcementResult> {
        let rb = self.check_bytes(key, bytes, tick)?;
        if rb.is_denied() {
            return Ok(rb);
        }
        let rbl = self.check_blobs(key, blobs, tick)?;
        if rbl.is_denied() {
            return Ok(rbl);
        }
        self.check_inodes(key, inodes, tick)
    }

    /// Applique le résultat : retourne `Err(QuotaExceeded)` si refusé.
    pub fn apply(&self, result: &EnforcementResult) -> ExofsResult<()> {
        if result.is_denied() {
            Err(ExofsError::QuotaExceeded)
        } else {
            Ok(())
        }
    }

    /// `check_write` puis `apply` en une seule opération.
    pub fn check_and_apply(
        &self,
        key: QuotaKey,
        bytes: u64,
        blobs: u64,
        inodes: u64,
        tick: u64,
    ) -> ExofsResult<()> {
        let r = self.check_write(key, bytes, blobs, inodes, tick)?;
        self.apply(&r)
    }

    // ── Helpers privés ────────────────────────────────────────────────────────

    fn _make_allow(&self, key: QuotaKey) -> EnforcementResult {
        EnforcementResult::allow(key)
    }

    fn _build(
        &self,
        key: QuotaKey,
        action: EnforcementAction,
        _used: u64,
        _limit: u64,
        requested: u64,
        _tick: u64,
    ) -> EnforcementResult {
        let usage = QUOTA_TRACKER.get_usage(key).unwrap_or(QuotaUsage::zero());
        let limits = QUOTA_TRACKER
            .get_limits(key)
            .unwrap_or(super::quota_policy::QuotaLimits::unlimited());
        EnforcementResult {
            action: action,
            entity_id: key.entity_id,
            used_bytes: usage.bytes_used,
            used_blobs: usage.blobs_used,
            used_inodes: usage.inodes_used,
            limit_bytes: limits.hard_bytes,
            limit_blobs: limits.hard_blobs,
            limit_inodes: limits.hard_inodes,
            grace_ticks: limits.grace_ticks,
            requested: requested,
        }
    }

    /// Collecte les clés en hard-dépassement (RECUR-01).
    pub fn hard_exceeded_keys(&self) -> ExofsResult<Vec<QuotaKey>> {
        QUOTA_TRACKER.hard_exceeded_keys()
    }

    /// Réinitialise les statistiques d'enforcement.
    pub fn reset_stats(&self) {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let s = unsafe { &mut *self.stats.get() };
        *s = EnforcementStats::zero();
        self.release();
    }
}

/// Singleton global du moteur d'enforcement.
pub static QUOTA_ENFORCER: QuotaEnforcer = QuotaEnforcer::new_const();

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::exofs::quota::quota_policy::{QuotaKind, QuotaLimits};
    use crate::fs::exofs::quota::quota_tracker::QUOTA_TRACKER;

    fn setup_key(entity_id: u64, hard_bytes: u64) -> QuotaKey {
        let key = QuotaKey::new(QuotaKind::User, entity_id);
        let mut limits = QuotaLimits::unlimited();
        limits.hard_bytes = hard_bytes;
        limits.soft_bytes = hard_bytes / 2;
        QUOTA_TRACKER.set_limits(key, limits).unwrap();
        QUOTA_TRACKER.reset_usage(key).unwrap_or(());
        key
    }

    #[test]
    fn test_allow_under_limit() {
        let enforcer = QuotaEnforcer::new_const();
        let key = setup_key(100, 10_000);
        let r = enforcer.check_bytes(key, 1_000, 0).unwrap();
        assert!(r.is_allowed());
        assert_eq!(r.action, EnforcementAction::Allow);
    }

    #[test]
    fn test_deny_over_hard_limit() {
        let enforcer = QuotaEnforcer::new_const();
        let key = setup_key(101, 1_000);
        // Remplir l'usage
        QUOTA_TRACKER.add_bytes(key, 900).unwrap();
        let r = enforcer.check_bytes(key, 200, 0).unwrap();
        assert!(r.is_denied());
        assert_eq!(r.action, EnforcementAction::Deny);
    }

    #[test]
    fn test_soft_warning() {
        let enforcer = QuotaEnforcer::new_const();
        let key = setup_key(102, 10_000);
        // Usage 6000 > soft 5000
        QUOTA_TRACKER.add_bytes(key, 5_500).unwrap();
        let r = enforcer.check_bytes(key, 600, 0).unwrap();
        assert!(r.is_allowed());
        assert_eq!(r.action, EnforcementAction::AllowWithWarning);
    }

    #[test]
    fn test_apply_deny_returns_quota_exceeded() {
        let enforcer = QuotaEnforcer::new_const();
        let result = EnforcementResult {
            action: EnforcementAction::Deny,
            entity_id: 0,
            used_bytes: 0,
            used_blobs: 0,
            used_inodes: 0,
            limit_bytes: 0,
            limit_blobs: 0,
            limit_inodes: 0,
            grace_ticks: 0,
            requested: 0,
        };
        assert!(matches!(
            enforcer.apply(&result),
            Err(ExofsError::QuotaExceeded)
        ));
    }

    #[test]
    fn test_apply_allow_ok() {
        let enforcer = QuotaEnforcer::new_const();
        let key = QuotaKey::new(QuotaKind::User, 0);
        let result = EnforcementResult::allow(key);
        assert!(enforcer.apply(&result).is_ok());
    }

    #[test]
    fn test_check_inodes_deny() {
        let enforcer = QuotaEnforcer::new_const();
        let key = QuotaKey::new(QuotaKind::User, 103);
        let mut limits = QuotaLimits::unlimited();
        limits.hard_inodes = 10;
        QUOTA_TRACKER.set_limits(key, limits).unwrap();
        QUOTA_TRACKER.add_inodes(key, 9).unwrap();
        let r = enforcer.check_inodes(key, 5, 0).unwrap();
        assert!(r.is_denied());
    }

    #[test]
    fn test_check_blobs_soft_warning() {
        let enforcer = QuotaEnforcer::new_const();
        let key = QuotaKey::new(QuotaKind::User, 104);
        let mut limits = QuotaLimits::unlimited();
        limits.hard_blobs = 100;
        limits.soft_blobs = 50;
        QUOTA_TRACKER.set_limits(key, limits).unwrap();
        QUOTA_TRACKER.add_blobs(key, 40).unwrap();
        let r = enforcer.check_blobs(key, 15, 0).unwrap();
        assert!(r.is_allowed());
        assert_eq!(r.action, EnforcementAction::AllowWithWarning);
    }

    #[test]
    fn test_check_and_apply_ok() {
        let enforcer = QuotaEnforcer::new_const();
        let key = setup_key(105, 100_000);
        assert!(enforcer.check_and_apply(key, 1_000, 1, 1, 0).is_ok());
    }

    #[test]
    fn test_check_and_apply_denied() {
        let enforcer = QuotaEnforcer::new_const();
        let key = setup_key(106, 100);
        assert!(enforcer.check_and_apply(key, 200, 0, 0, 0).is_err());
    }

    #[test]
    fn test_stats_tracking() {
        let enforcer = QuotaEnforcer::new_const();
        let key = setup_key(107, 10_000);
        enforcer.check_bytes(key, 100, 0).unwrap();
        enforcer.check_bytes(key, 100, 0).unwrap();
        let s = enforcer.stats();
        assert_eq!(s.checks_total, 2);
        assert_eq!(s.allowed, 2);
    }

    #[test]
    fn test_enforcement_action_reason() {
        assert_eq!(EnforcementAction::Allow.reason_str(), "allowed");
        assert_eq!(EnforcementAction::Deny.reason_str(), "denied-hard");
        assert!(EnforcementAction::Deny.is_deny());
        assert!(EnforcementAction::Allow.is_allow());
    }

    #[test]
    fn test_bytes_ppt() {
        let key = QuotaKey::new(QuotaKind::User, 0);
        let mut r = EnforcementResult::allow(key);
        r.used_bytes = 750;
        r.limit_bytes = 1000;
        assert_eq!(r.bytes_ppt(), 750);
    }

    #[test]
    fn test_violation_overshoot() {
        let key = QuotaKey::new(QuotaKind::User, 0);
        let v = QuotaViolation::new(key, QuotaEvent::HardDenial, 200, 900, 1000, 0);
        assert_eq!(v.overshoot_bytes(), 100);
    }
}
