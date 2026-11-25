//! ExoFS snapshot subsystem lifecycle (`init`, `shutdown`, `verify_health`, test reset).
//!
//! Extracted from `exofs/snapshot/mod.rs` so `mod.rs` remains a pure wiring facade.

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
extern crate std;

use super::{SNAPSHOT_LIST, SNAPSHOT_MOUNT, SNAPSHOT_PROTECT, SNAPSHOT_QUOTA};

pub fn init() {}

pub fn shutdown() {
    SNAPSHOT_LIST.clear();
    SNAPSHOT_PROTECT.clear();
    SNAPSHOT_MOUNT.clear();
    SNAPSHOT_QUOTA.clear();
}

#[cfg(test)]
static SNAPSHOT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub fn reset_for_test() -> std::sync::MutexGuard<'static, ()> {
    let guard = SNAPSHOT_TEST_LOCK.lock().test_unwrap();
    shutdown();
    guard
}

pub fn verify_health() -> HealthReport {
    HealthReport {
        list_ok: SNAPSHOT_LIST.verify_consistency().is_ok(),
        n_snaps: SNAPSHOT_LIST.count(),
        n_mounts: SNAPSHOT_MOUNT.n_mounts(),
        n_protected: SNAPSHOT_PROTECT.n_protected(),
        n_worm: SNAPSHOT_PROTECT.n_worm(),
    }
}

#[derive(Debug, Clone, Copy)]
pub struct HealthReport {
    pub list_ok: bool,
    pub n_snaps: usize,
    pub n_mounts: usize,
    pub n_protected: usize,
    pub n_worm: usize,
}

impl HealthReport {
    pub fn is_healthy(&self) -> bool {
        self.list_ok
    }
}
