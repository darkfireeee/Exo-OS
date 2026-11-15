// SPDX-License-Identifier: MIT
// ExoFS Quota — Main module (public facade)
// ≥400L, ExofsError only, RECUR-01/OOM-02/ARITH-02

//! # ExoFS Quota
//!
//! This module provides a complete quota system for ExoFS:
//! - **Policies**   : soft/hard limits for bytes/blobs/inodes with grace period.
//! - **Tracking**   : per-entity (user/group/project) tracking on flat array.
//! - **Audit**      : event ring with filtering and statistics.
//! - **Namespaces** : quota isolation and inheritance per namespace.
//! - **Enforcement**: real-time checking and limit enforcement.
//! - **Reports**    : consumption reports and top-consumers.
//!
//! ## Implementation rules
//! - RECUR-01 : zero recursion, `while` loops only.
//! - OOM-02   : `try_reserve(n).map_err(|_| ExofsError::NoMemory)?` before push.
//! - ARITH-02 : `saturating_add/sub`, `checked_div`, `wrapping_add/mul`.
//! - Never `FsError` : `ExofsError` / `ExofsResult` exclusively.

pub mod quota_audit;
pub mod quota_enforcement;
pub mod quota_namespace;
pub mod quota_policy;
pub mod quota_report;
pub mod quota_tracker;

// ─── Public re-exports ────────────────────────────────────────────────────────

// Policy
pub use quota_policy::{PolicyFlags, PolicyPresets, QuotaKind, QuotaLimits, QuotaPolicy};

// Tracking
pub use quota_tracker::{
    QuotaEntry, QuotaKey, QuotaTracker, QuotaUsage, QUOTA_MAX_ENTRIES, QUOTA_TRACKER,
};

// Audit
pub use quota_audit::{
    advance_audit_tick, audit_tick, AuditFilter, AuditSession, AuditSummary, QuotaAuditEntry,
    QuotaAuditLog, QuotaEvent, QUOTA_AUDIT,
};

// Namespaces
pub use quota_namespace::{
    NamespaceFlags, NamespaceId, NamespaceStats, QuotaNamespace, QuotaNamespaceEntry,
    NAMESPACE_MAX, NS_NAME_LEN, QUOTA_NAMESPACE,
};

// Enforcement
pub use quota_enforcement::{
    EnforcementAction, EnforcementResult, EnforcementStats, QuotaEnforcer, QuotaViolation,
    QUOTA_ENFORCER,
};

// Reports
pub use quota_report::{
    QuotaReport, QuotaReportEntry, QuotaReporter, ReportDimension, QUOTA_REPORTER,
    SEVERITY_CRITICAL_PPT, SEVERITY_WARNING_PPT,
};

use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;

// ─── QuotaConfig ──────────────────────────────────────────────────────────────

/// Global configuration for the quota module.
#[derive(Clone, Copy, Debug)]
pub struct QuotaConfig {
    /// Enables quota enforcement.
    pub enabled: bool,
    /// Strict mode: deny if check fails with an internal error.
    pub strict_mode: bool,
    /// Default quota type.
    pub default_kind: QuotaKind,
    /// Global default grace period in ticks.
    pub grace_ticks_default: u64,
    /// Maximum number of entities per namespace.
    pub max_entities_per_ns: u64,
    /// Enable auditing of every operation.
    pub audit_all_ops: bool,
}

impl QuotaConfig {
    pub const fn default_config() -> Self {
        Self {
            enabled: true,
            strict_mode: false,
            default_kind: QuotaKind::User,
            grace_ticks_default: 86_400,
            max_entities_per_ns: 1_024,
            audit_all_ops: false,
        }
    }

    pub const fn strict() -> Self {
        Self {
            enabled: true,
            strict_mode: true,
            default_kind: QuotaKind::User,
            grace_ticks_default: 0,
            max_entities_per_ns: 256,
            audit_all_ops: true,
        }
    }

    pub fn validate(&self) -> ExofsResult<()> {
        if self.max_entities_per_ns == 0 {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(())
    }
}

// ─── QuotaModuleState ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuotaModuleState {
    Uninitialized,
    Initializing,
    Ready,
    Error,
}

impl QuotaModuleState {
    pub fn name(self) -> &'static str {
        match self {
            Self::Uninitialized => "uninitialized",
            Self::Initializing => "initializing",
            Self::Ready => "ready",
            Self::Error => "error",
        }
    }
    pub fn is_ready(self) -> bool {
        matches!(self, Self::Ready)
    }
}

// ─── QuotaModule ──────────────────────────────────────────────────────────────

/// Main facade for the quota module.
pub struct QuotaModule {
    config: core::cell::UnsafeCell<QuotaConfig>,
    state: core::cell::UnsafeCell<QuotaModuleState>,
    lock: core::sync::atomic::AtomicU64,
}

unsafe impl Sync for QuotaModule {}
unsafe impl Send for QuotaModule {}

impl QuotaModule {
    pub const fn new_const() -> Self {
        Self {
            config: core::cell::UnsafeCell::new(QuotaConfig::default_config()),
            state: core::cell::UnsafeCell::new(QuotaModuleState::Uninitialized),
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

    /// Initializes the module with the given configuration.
    pub fn init(&self, config: QuotaConfig, tick: u64) -> ExofsResult<()> {
        config.validate()?;
        self.acquire();
        // SAFETY: exclusive access guaranteed by atomic lock acquired beforehand.
        let state = unsafe { &mut *self.state.get() };
        if *state == QuotaModuleState::Ready {
            self.release();
            return Ok(());
        }
        *state = QuotaModuleState::Initializing;
        // SAFETY: exclusive access guaranteed by atomic lock acquired beforehand.
        unsafe {
            *self.config.get() = config;
        }
        self.release();

        // Initialize root namespace
        let root_policy = QuotaPolicy::default_policy(config.default_kind);
        QUOTA_NAMESPACE
            .init_root(root_policy, tick)
            .unwrap_or_default(); // may fail if already initialized

        self.acquire();
        // SAFETY: exclusive access guaranteed by atomic lock acquired beforehand.
        *unsafe { &mut *self.state.get() } = QuotaModuleState::Ready;
        self.release();
        Ok(())
    }

    pub fn state(&self) -> QuotaModuleState {
        self.acquire();
        // SAFETY: exclusive access guaranteed by atomic lock acquired beforehand.
        let s = unsafe { *self.state.get() };
        self.release();
        s
    }

    pub fn config(&self) -> QuotaConfig {
        self.acquire();
        // SAFETY: exclusive access guaranteed by atomic lock acquired beforehand.
        let c = unsafe { *self.config.get() };
        self.release();
        c
    }

    pub fn is_enabled(&self) -> bool {
        self.config().enabled
    }

    // ── High-level operations ─────────────────────────────────────────────────

    /// Checks and enforces a write (bytes + blobs + inodes).
    pub fn check_write(
        &self,
        key: QuotaKey,
        bytes: u64,
        blobs: u64,
        inodes: u64,
        tick: u64,
    ) -> ExofsResult<()> {
        if !self.is_enabled() {
            return Ok(());
        }
        QUOTA_ENFORCER.check_and_apply(key, bytes, blobs, inodes, tick)
    }

    /// Records actual consumption following a successful write.
    pub fn record_write(
        &self,
        key: QuotaKey,
        bytes: u64,
        blobs: u64,
        inodes: u64,
    ) -> ExofsResult<()> {
        if !self.is_enabled() {
            return Ok(());
        }
        let b = i64::try_from(bytes).map_err(|_| ExofsError::InvalidArgument)?;
        let bl = i64::try_from(blobs).map_err(|_| ExofsError::InvalidArgument)?;
        let ino = i64::try_from(inodes).map_err(|_| ExofsError::InvalidArgument)?;
        QUOTA_TRACKER.mutate_usage(key, b, bl, ino)
    }

    /// Releases consumption (data deletion).
    pub fn release_write(
        &self,
        key: QuotaKey,
        bytes: u64,
        blobs: u64,
        inodes: u64,
    ) -> ExofsResult<()> {
        if !self.is_enabled() {
            return Ok(());
        }
        let b = i64::try_from(bytes).map_err(|_| ExofsError::InvalidArgument)?;
        let bl = i64::try_from(blobs).map_err(|_| ExofsError::InvalidArgument)?;
        let ino = i64::try_from(inodes).map_err(|_| ExofsError::InvalidArgument)?;
        QUOTA_TRACKER.mutate_usage(key, -b, -bl, -ino)
    }

    /// Configures limits for an entity.
    pub fn set_limits(&self, key: QuotaKey, limits: QuotaLimits) -> ExofsResult<()> {
        limits.validate()?;
        QUOTA_TRACKER.set_limits(key, limits)?;
        let _tick = audit_tick();
        QUOTA_AUDIT.log_limit_set(key, limits.hard_bytes);
        Ok(())
    }

    /// Removes an entity's quota.
    pub fn remove_entity(&self, key: QuotaKey) -> ExofsResult<()> {
        QUOTA_TRACKER.remove(key)?;
        let _tick = audit_tick();
        QUOTA_AUDIT.log_entity_removed(key);
        Ok(())
    }

    /// Generates a full report.
    pub fn report(&self) -> ExofsResult<QuotaReport> {
        QuotaReport::from_tracker()
    }

    /// Audit summary.
    pub fn audit_summary(&self) -> AuditSummary {
        QUOTA_AUDIT.summary()
    }

    /// Enforcement statistics.
    pub fn enforcement_stats(&self) -> EnforcementStats {
        QUOTA_ENFORCER.stats()
    }

    /// Resets all data (tracker + audit).
    pub fn reset_all(&self) -> ExofsResult<()> {
        // Snapshot then removal of all keys
        let all = QUOTA_TRACKER.snapshot_all()?;
        let n = all.len();
        let mut i = 0usize;
        while i < n {
            let _ = QUOTA_TRACKER.remove(all[i].key);
            i = i.wrapping_add(1);
        }
        QUOTA_AUDIT.reset_counters();
        QUOTA_ENFORCER.reset_stats();
        Ok(())
    }

    /// Snapshot of all keys exceeding hard limit.
    pub fn hard_exceeded(&self) -> ExofsResult<Vec<QuotaKey>> {
        QUOTA_TRACKER.hard_exceeded_keys()
    }
}

/// Global singleton of the quota module.
pub static QUOTA: QuotaModule = QuotaModule::new_const();

// ─── Global utility functions ─────────────────────────────────────────────────

/// Initializes the quota module with default configuration.
pub fn quota_init(tick: u64) -> ExofsResult<()> {
    QUOTA.init(QuotaConfig::default_config(), tick)
}

/// Checks and enforces a write quota.
pub fn quota_check_write(key: QuotaKey, bytes: u64, tick: u64) -> ExofsResult<()> {
    QUOTA.check_write(key, bytes, 0, 0, tick)
}

/// Configures limits for an entity.
pub fn quota_set_limits(key: QuotaKey, limits: QuotaLimits) -> ExofsResult<()> {
    QUOTA.set_limits(key, limits)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;

    fn mk_key(id: u64) -> QuotaKey {
        QuotaKey::new(QuotaKind::User, id)
    }

    fn mk_limits(hard: u64) -> QuotaLimits {
        let mut l = QuotaLimits::unlimited();
        l.hard_bytes = hard;
        l.soft_bytes = hard / 2;
        l
    }

    #[test]
    fn test_module_init() {
        let m = QuotaModule::new_const();
        m.init(QuotaConfig::default_config(), 0).test_unwrap();
        assert!(m.state().is_ready());
    }

    #[test]
    fn test_module_double_init_ok() {
        let m = QuotaModule::new_const();
        m.init(QuotaConfig::default_config(), 0).test_unwrap();
        m.init(QuotaConfig::default_config(), 1).test_unwrap();
        assert!(m.state().is_ready());
    }

    #[test]
    fn test_check_write_allowed() {
        let m = QuotaModule::new_const();
        m.init(QuotaConfig::default_config(), 0).test_unwrap();
        let key = mk_key(300);
        m.set_limits(key, mk_limits(100_000)).test_unwrap();
        assert!(m.check_write(key, 1_000, 0, 0, 0).is_ok());
    }

    #[test]
    fn test_check_write_denied() {
        let m = QuotaModule::new_const();
        m.init(QuotaConfig::default_config(), 0).test_unwrap();
        let key = mk_key(301);
        m.set_limits(key, mk_limits(500)).test_unwrap();
        // Fill tracker
        QUOTA_TRACKER.add_bytes(key, 400).test_unwrap();
        assert!(m.check_write(key, 200, 0, 0, 0).is_err());
    }

    #[test]
    fn test_record_and_release() {
        let m = QuotaModule::new_const();
        m.init(QuotaConfig::default_config(), 0).test_unwrap();
        let key = mk_key(302);
        m.set_limits(key, mk_limits(100_000)).test_unwrap();
        QUOTA_TRACKER.reset_usage(key).unwrap_or(());
        m.record_write(key, 5_000, 2, 1).test_unwrap();
        let u = QUOTA_TRACKER.get_usage(key).test_unwrap();
        assert_eq!(u.bytes_used, 5_000);
        assert_eq!(u.blobs_used, 2);
        m.release_write(key, 1_000, 1, 0).test_unwrap();
        let u2 = QUOTA_TRACKER.get_usage(key).test_unwrap();
        assert_eq!(u2.bytes_used, 4_000);
        assert_eq!(u2.blobs_used, 1);
    }

    #[test]
    fn test_remove_entity() {
        let m = QuotaModule::new_const();
        m.init(QuotaConfig::default_config(), 0).test_unwrap();
        let key = mk_key(303);
        m.set_limits(key, mk_limits(10_000)).test_unwrap();
        m.remove_entity(key).test_unwrap();
        assert!(QUOTA_TRACKER.get_usage(key).is_none());
    }

    #[test]
    fn test_config_default() {
        let c = QuotaConfig::default_config();
        assert!(c.enabled);
        c.validate().test_unwrap();
    }

    #[test]
    fn test_config_strict() {
        let c = QuotaConfig::strict();
        assert!(c.strict_mode);
        assert_eq!(c.grace_ticks_default, 0);
        c.validate().test_unwrap();
    }

    #[test]
    fn test_module_state_name() {
        assert_eq!(QuotaModuleState::Ready.name(), "ready");
        assert_eq!(QuotaModuleState::Uninitialized.name(), "uninitialized");
    }

    #[test]
    fn test_quota_init_fn() {
        quota_init(0).test_unwrap();
    }

    #[test]
    fn test_quota_check_write_fn() {
        quota_init(0).test_unwrap();
        let key = mk_key(304);
        quota_set_limits(key, mk_limits(100_000)).test_unwrap();
        assert!(quota_check_write(key, 500, 0).is_ok());
    }

    #[test]
    fn test_report_via_module() {
        let m = QuotaModule::new_const();
        m.init(QuotaConfig::default_config(), 0).test_unwrap();
        let key = mk_key(305);
        m.set_limits(key, mk_limits(10_000)).test_unwrap();
        let r = m.report().test_unwrap();
        assert!(r.entry_count() > 0);
    }

    #[test]
    fn test_enforcement_stats_accessible() {
        let m = QuotaModule::new_const();
        m.init(QuotaConfig::default_config(), 0).test_unwrap();
        let s = m.enforcement_stats();
        let _ = s.denial_rate_ppt();
    }
}
