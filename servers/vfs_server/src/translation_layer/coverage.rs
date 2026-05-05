//! Coverage accounting for the POSIX translation contract.

use super::posix_services::{CORE_POSIX_SERVICES, PHASE2_POSIX_SERVICES};

pub const CORE_TARGET_PERCENT: u32 = 100;
pub const PHASE2_TARGET_PERCENT: u32 = 100;

#[derive(Clone, Copy, Debug, Default)]
pub struct CoverageSummary {
    pub core_total: u32,
    pub core_supported: u32,
    pub phase2_total: u32,
    pub phase2_supported: u32,
}

impl CoverageSummary {
    pub const fn core_percent(self) -> u32 {
        if self.core_total == 0 {
            0
        } else {
            self.core_supported.saturating_mul(100) / self.core_total
        }
    }

    pub const fn phase2_percent(self) -> u32 {
        let total = self.core_total.saturating_add(self.phase2_total);
        if total == 0 {
            0
        } else {
            self.core_supported
                .saturating_add(self.phase2_supported)
                .saturating_mul(100)
                / total
        }
    }
}

pub fn coverage_summary() -> CoverageSummary {
    let mut core_supported = 0u32;
    let mut i = 0usize;
    while i < CORE_POSIX_SERVICES.len() {
        if CORE_POSIX_SERVICES[i].status.counts_for_core() {
            core_supported = core_supported.saturating_add(1);
        }
        i += 1;
    }

    CoverageSummary {
        core_total: CORE_POSIX_SERVICES.len() as u32,
        core_supported,
        phase2_total: PHASE2_POSIX_SERVICES.len() as u32,
        phase2_supported: 0,
    }
}

pub fn meets_core_target() -> bool {
    coverage_summary().core_percent() >= CORE_TARGET_PERCENT
}
