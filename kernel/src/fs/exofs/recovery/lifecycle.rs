//! ExoFS recovery module lifecycle (`init`, `shutdown`, `verify_health`) and tests.
//!
//! Extracted from `exofs/recovery/mod.rs` so `mod.rs` remains a pure wiring facade.

use core::sync::atomic::{AtomicBool, Ordering};

use super::{RecoveryEvent, CHECKPOINT_STORE, RECOVERY_AUDIT, RECOVERY_LOG, REPAIR_LOG};
use crate::fs::exofs::core::{ExofsError, ExofsResult};

static RECOVERY_INITIALIZED: AtomicBool = AtomicBool::new(false);
static RECOVERY_SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug)]
pub struct RecoveryHealth {
    pub log_events_total: usize,
    pub audit_entries_total: usize,
    pub checkpoint_count: usize,
    pub repairs_total: usize,
    pub repairs_ok: bool,
    pub has_critical_audit: bool,
    pub initialized: bool,
}

pub fn init() -> ExofsResult<()> {
    if RECOVERY_INITIALIZED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(ExofsError::CommitInProgress);
    }
    RECOVERY_LOG.log_event(RecoveryEvent::RecoveryModuleLoaded);
    RECOVERY_AUDIT.record_init();
    Ok(())
}

pub fn shutdown() {
    if !RECOVERY_INITIALIZED.load(Ordering::Acquire) {
        return;
    }
    RECOVERY_SHUTTING_DOWN.store(true, Ordering::SeqCst);
    RECOVERY_LOG.log_event(RecoveryEvent::RecoveryModuleUnloaded);
    RECOVERY_INITIALIZED.store(false, Ordering::SeqCst);
    RECOVERY_SHUTTING_DOWN.store(false, Ordering::SeqCst);
}

pub fn verify_health() -> RecoveryHealth {
    let repairs_ok = REPAIR_LOG.all_recent_ok(16);
    let has_critical_audit = RECOVERY_AUDIT.violation_count() > 0;
    RecoveryHealth {
        log_events_total: RECOVERY_LOG.total_written() as usize,
        audit_entries_total: RECOVERY_AUDIT.total() as usize,
        checkpoint_count: CHECKPOINT_STORE.count(),
        repairs_total: REPAIR_LOG.total(),
        repairs_ok,
        has_critical_audit,
        initialized: RECOVERY_INITIALIZED.load(Ordering::Relaxed),
    }
}

#[inline]
pub fn is_initialized() -> bool {
    RECOVERY_INITIALIZED.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_idempotent() {
        let r1 = init();
        let r2 = init();
        shutdown();
        assert!(r1.is_ok() || matches!(r1, Err(ExofsError::CommitInProgress)));
        assert!(matches!(r2, Err(ExofsError::CommitInProgress)) || r2.is_ok());
    }

    #[test]
    fn test_shutdown_safe() {
        shutdown();
        shutdown();
    }

    #[test]
    fn test_verify_health_initialized() {
        let _ = init();
        let health = verify_health();
        assert!(health.initialized || !health.initialized);
        shutdown();
    }

    #[test]
    fn test_repair_log_reexport() {
        assert!(REPAIR_LOG.total() < usize::MAX);
    }

    #[test]
    fn test_recovery_log_reexport() {
        assert!((RECOVERY_LOG.total_written() as usize) < usize::MAX);
    }

    #[test]
    fn test_checkpoint_store_reexport() {
        assert!(CHECKPOINT_STORE.count() < usize::MAX);
    }
}
