// SPDX-License-Identifier: MIT
// ExoFS Quota — Module principal (façade publique)
// ≥400L, ExofsError only, RECUR-01/OOM-02/ARITH-02

//! # ExoFS Quota
//!
//! Ce module fournit un système de quota complet pour ExoFS :
//! - **Politiques** : limites soft/hard bytes/blobs/inodes avec délai de grâce.
//! - **Tracking**   : suivi par entité (user/group/project) sur tableau plat.
//! - **Audit**      : anneau d'événements avec filtrage et statistiques.
//! - **Namespaces** : isolation et héritage de quota par namespace.
//! - **Enforcement**: vérification temps réel et application des limites.
//! - **Reports**    : rapports de consommation et top-consumers.
//!
//! ## Règles d'implémentation
//! - RECUR-01 : aucune récursion, boucles `while` uniquement.
//! - OOM-02   : `try_reserve(n).map_err(|_| ExofsError::NoMemory)?` avant push.
//! - ARITH-02 : `saturating_add/sub`, `checked_div`, `wrapping_add/mul`.
//! - Jamais `FsError` : uniquement `ExofsError` / `ExofsResult`.

pub mod quota_policy;
pub mod quota_tracker;
pub mod quota_audit;
pub mod quota_namespace;
pub mod quota_enforcement;
pub mod quota_report;

// ─── Réexports publics ────────────────────────────────────────────────────────

// Politique
pub use quota_policy::{
    QuotaKind, QuotaLimits, PolicyFlags, QuotaPolicy, PolicyPresets,
};

// Tracking
pub use quota_tracker::{
    QuotaKey, QuotaUsage, QuotaEntry, QuotaTracker,
    QUOTA_TRACKER, QUOTA_MAX_ENTRIES,
};

// Audit
pub use quota_audit::{
    QuotaEvent, QuotaAuditEntry, AuditFilter, QuotaAuditLog,
    AuditSummary, AuditSession, QUOTA_AUDIT, audit_tick, advance_audit_tick,
};

// Namespaces
pub use quota_namespace::{
    NamespaceId, NamespaceFlags, QuotaNamespaceEntry, QuotaNamespace,
    NamespaceStats, QUOTA_NAMESPACE, NAMESPACE_MAX, NS_NAME_LEN,
};

// Enforcement
pub use quota_enforcement::{
    EnforcementAction, EnforcementResult, QuotaViolation,
    EnforcementStats, QuotaEnforcer, QUOTA_ENFORCER,
};

// Reports
pub use quota_report::{
    ReportDimension, QuotaReportEntry, QuotaReport, QuotaReporter,
    QUOTA_REPORTER, SEVERITY_WARNING_PPT, SEVERITY_CRITICAL_PPT,
};

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─── QuotaConfig ──────────────────────────────────────────────────────────────

/// Configuration globale du module quota.
#[derive(Clone, Copy, Debug)]
pub struct QuotaConfig {
    /// Active l'enforcement des quotas.
    pub enabled:              bool,
    /// Mode strict : refuser si la vérification échoue avec une erreur interne.
    pub strict_mode:          bool,
    /// Type de quota par défaut.
    pub default_kind:         QuotaKind,
    /// Délai de grâce global en ticks.
    pub grace_ticks_default:  u64,
    /// Nombre maximal d'entités par namespace.
    pub max_entities_per_ns:  u64,
    /// Activer l'audit de chaque opération.
    pub audit_all_ops:        bool,
}

impl QuotaConfig {
    pub const fn default_config() -> Self {
        Self {
            enabled:             true,
            strict_mode:         false,
            default_kind:        QuotaKind::User,
            grace_ticks_default: 86_400,
            max_entities_per_ns: 1_024,
            audit_all_ops:       false,
        }
    }

    pub const fn strict() -> Self {
        Self {
            enabled:             true,
            strict_mode:         true,
            default_kind:        QuotaKind::User,
            grace_ticks_default: 0,
            max_entities_per_ns: 256,
            audit_all_ops:       true,
        }
    }

    pub fn validate(&self) -> ExofsResult<()> {
        if self.max_entities_per_ns == 0 { return Err(ExofsError::InvalidArgument); }
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
            Self::Initializing  => "initializing",
            Self::Ready         => "ready",
            Self::Error         => "error",
        }
    }
    pub fn is_ready(self) -> bool { matches!(self, Self::Ready) }
}

// ─── QuotaModule ──────────────────────────────────────────────────────────────

/// Façade principale du module quota.
pub struct QuotaModule {
    config: core::cell::UnsafeCell<QuotaConfig>,
    state:  core::cell::UnsafeCell<QuotaModuleState>,
    lock:   core::sync::atomic::AtomicU64,
}

unsafe impl Sync for QuotaModule {}
unsafe impl Send for QuotaModule {}

impl QuotaModule {
    pub const fn new_const() -> Self {
        Self {
            config: core::cell::UnsafeCell::new(QuotaConfig::default_config()),
            state:  core::cell::UnsafeCell::new(QuotaModuleState::Uninitialized),
            lock:   core::sync::atomic::AtomicU64::new(0),
        }
    }

    fn acquire(&self) {
        use core::sync::atomic::Ordering;
        while self.lock
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

    /// Initialise le module avec la configuration donnée.
    pub fn init(&self, config: QuotaConfig, tick: u64) -> ExofsResult<()> {
        config.validate()?;
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let state = unsafe { &mut *self.state.get() };
        if *state == QuotaModuleState::Ready {
            self.release();
            return Ok(());
        }
        *state = QuotaModuleState::Initializing;
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        unsafe { *self.config.get() = config; }
        self.release();

        // Initialiser le namespace root
        let root_policy = QuotaPolicy::default_policy(config.default_kind);
        QUOTA_NAMESPACE.init_root(root_policy, tick)
            .unwrap_or_default(); // peut échouer si déjà initialisé

        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        *unsafe { &mut *self.state.get() } = QuotaModuleState::Ready;
        self.release();
        Ok(())
    }

    pub fn state(&self) -> QuotaModuleState {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let s = unsafe { *self.state.get() };
        self.release();
        s
    }

    pub fn config(&self) -> QuotaConfig {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let c = unsafe { *self.config.get() };
        self.release();
        c
    }

    pub fn is_enabled(&self) -> bool { self.config().enabled }

    // ── Opérations de haut niveau ─────────────────────────────────────────────

    /// Vérifie et applique une écriture (bytes + blobs + inodes).
    pub fn check_write(
        &self, key: QuotaKey, bytes: u64, blobs: u64, inodes: u64, tick: u64
    ) -> ExofsResult<()> {
        if !self.is_enabled() { return Ok(()); }
        QUOTA_ENFORCER.check_and_apply(key, bytes, blobs, inodes, tick)
    }

    /// Enregistre une consommation effective après une écriture réussie.
    pub fn record_write(
        &self, key: QuotaKey, bytes: u64, blobs: u64, inodes: u64
    ) -> ExofsResult<()> {
        if !self.is_enabled() { return Ok(()); }
        if bytes  > 0 { QUOTA_TRACKER.add_bytes(key, bytes)?; }
        if blobs  > 0 { QUOTA_TRACKER.add_blobs(key, blobs)?; }
        if inodes > 0 { QUOTA_TRACKER.add_inodes(key, inodes)?; }
        Ok(())
    }

    /// Libère une consommation (suppression de données).
    pub fn release_write(
        &self, key: QuotaKey, bytes: u64, blobs: u64, inodes: u64
    ) -> ExofsResult<()> {
        if !self.is_enabled() { return Ok(()); }
        if bytes  > 0 { QUOTA_TRACKER.sub_bytes(key, bytes)?; }
        if blobs  > 0 { QUOTA_TRACKER.sub_blobs(key, blobs)?; }
        if inodes > 0 { QUOTA_TRACKER.sub_inodes(key, inodes)?; }
        Ok(())
    }

    /// Configure les limites d'une entité.
    pub fn set_limits(&self, key: QuotaKey, limits: QuotaLimits) -> ExofsResult<()> {
        limits.validate()?;
        QUOTA_TRACKER.set_limits(key, limits)?;
        let _tick = audit_tick();
        QUOTA_AUDIT.log_limit_set(key, limits.hard_bytes);
        Ok(())
    }

    /// Supprime le quota d'une entité.
    pub fn remove_entity(&self, key: QuotaKey) -> ExofsResult<()> {
        QUOTA_TRACKER.remove(key)?;
        let _tick = audit_tick();
        QUOTA_AUDIT.log_entity_removed(key);
        Ok(())
    }

    /// Génère un rapport complet.
    pub fn report(&self) -> ExofsResult<QuotaReport> {
        QuotaReport::from_tracker()
    }

    /// Résumé de l'audit.
    pub fn audit_summary(&self) -> AuditSummary {
        QUOTA_AUDIT.summary()
    }

    /// Statistiques d'enforcement.
    pub fn enforcement_stats(&self) -> EnforcementStats {
        QUOTA_ENFORCER.stats()
    }

    /// Réinitialise toutes les données (tracker + audit).
    pub fn reset_all(&self) -> ExofsResult<()> {
        // Snapshot puis suppression de toutes les clés
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

    /// Snapshot de toutes les clés en dépassement hard.
    pub fn hard_exceeded(&self) -> ExofsResult<Vec<QuotaKey>> {
        QUOTA_TRACKER.hard_exceeded_keys()
    }
}

/// Singleton global du module quota.
pub static QUOTA: QuotaModule = QuotaModule::new_const();

// ─── Fonctions utilitaires globales ───────────────────────────────────────────

/// Initialise le module quota avec la configuration par défaut.
pub fn quota_init(tick: u64) -> ExofsResult<()> {
    QUOTA.init(QuotaConfig::default_config(), tick)
}

/// Vérifie et aplique un quota d'écriture.
pub fn quota_check_write(key: QuotaKey, bytes: u64, tick: u64) -> ExofsResult<()> {
    QUOTA.check_write(key, bytes, 0, 0, tick)
}

/// Configure les limites d'une entité.
pub fn quota_set_limits(key: QuotaKey, limits: QuotaLimits) -> ExofsResult<()> {
    QUOTA.set_limits(key, limits)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_key(id: u64) -> QuotaKey { QuotaKey::new(QuotaKind::User, id) }

    fn mk_limits(hard: u64) -> QuotaLimits {
        let mut l = QuotaLimits::unlimited();
        l.hard_bytes = hard;
        l.soft_bytes = hard / 2;
        l
    }

    #[test]
    fn test_module_init() {
        let m = QuotaModule::new_const();
        m.init(QuotaConfig::default_config(), 0).unwrap();
        assert!(m.state().is_ready());
    }

    #[test]
    fn test_module_double_init_ok() {
        let m = QuotaModule::new_const();
        m.init(QuotaConfig::default_config(), 0).unwrap();
        m.init(QuotaConfig::default_config(), 1).unwrap();
        assert!(m.state().is_ready());
    }

    #[test]
    fn test_check_write_allowed() {
        let m = QuotaModule::new_const();
        m.init(QuotaConfig::default_config(), 0).unwrap();
        let key = mk_key(300);
        m.set_limits(key, mk_limits(100_000)).unwrap();
        assert!(m.check_write(key, 1_000, 0, 0, 0).is_ok());
    }

    #[test]
    fn test_check_write_denied() {
        let m = QuotaModule::new_const();
        m.init(QuotaConfig::default_config(), 0).unwrap();
        let key = mk_key(301);
        m.set_limits(key, mk_limits(500)).unwrap();
        // Remplir le tracker
        QUOTA_TRACKER.add_bytes(key, 400).unwrap();
        assert!(m.check_write(key, 200, 0, 0, 0).is_err());
    }

    #[test]
    fn test_record_and_release() {
        let m = QuotaModule::new_const();
        m.init(QuotaConfig::default_config(), 0).unwrap();
        let key = mk_key(302);
        m.set_limits(key, mk_limits(100_000)).unwrap();
        QUOTA_TRACKER.reset_usage(key).unwrap_or(());
        m.record_write(key, 5_000, 2, 1).unwrap();
        let u = QUOTA_TRACKER.get_usage(key).unwrap();
        assert_eq!(u.bytes_used, 5_000);
        assert_eq!(u.blobs_used, 2);
        m.release_write(key, 1_000, 1, 0).unwrap();
        let u2 = QUOTA_TRACKER.get_usage(key).unwrap();
        assert_eq!(u2.bytes_used, 4_000);
        assert_eq!(u2.blobs_used, 1);
    }

    #[test]
    fn test_remove_entity() {
        let m = QuotaModule::new_const();
        m.init(QuotaConfig::default_config(), 0).unwrap();
        let key = mk_key(303);
        m.set_limits(key, mk_limits(10_000)).unwrap();
        m.remove_entity(key).unwrap();
        assert!(QUOTA_TRACKER.get_usage(key).is_none());
    }

    #[test]
    fn test_config_default() {
        let c = QuotaConfig::default_config();
        assert!(c.enabled);
        c.validate().unwrap();
    }

    #[test]
    fn test_config_strict() {
        let c = QuotaConfig::strict();
        assert!(c.strict_mode);
        assert_eq!(c.grace_ticks_default, 0);
        c.validate().unwrap();
    }

    #[test]
    fn test_module_state_name() {
        assert_eq!(QuotaModuleState::Ready.name(), "ready");
        assert_eq!(QuotaModuleState::Uninitialized.name(), "uninitialized");
    }

    #[test]
    fn test_quota_init_fn() {
        quota_init(0).unwrap();
    }

    #[test]
    fn test_quota_check_write_fn() {
        quota_init(0).unwrap();
        let key = mk_key(304);
        quota_set_limits(key, mk_limits(100_000)).unwrap();
        assert!(quota_check_write(key, 500, 0).is_ok());
    }

    #[test]
    fn test_report_via_module() {
        let m = QuotaModule::new_const();
        m.init(QuotaConfig::default_config(), 0).unwrap();
        let key = mk_key(305);
        m.set_limits(key, mk_limits(10_000)).unwrap();
        let r = m.report().unwrap();
        assert!(r.entry_count() > 0);
    }

    #[test]
    fn test_enforcement_stats_accessible() {
        let m = QuotaModule::new_const();
        m.init(QuotaConfig::default_config(), 0).unwrap();
        let s = m.enforcement_stats();
        let _ = s.denial_rate_ppt();
    }
}
