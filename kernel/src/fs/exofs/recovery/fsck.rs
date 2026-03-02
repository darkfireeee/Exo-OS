//! Fsck — orchestrateur des 4 phases de vérification ExoFS (no_std).

use crate::fs::exofs::core::FsError;
use super::boot_recovery::BlockDevice;
use super::fsck_phase1::Phase1Checker;
use super::fsck_phase2::Phase2Checker;
use super::fsck_phase3::Phase3Checker;
use super::fsck_phase4::Phase4Checker;
use super::fsck_repair::FsckRepair;
use super::recovery_log::RECOVERY_LOG;
use super::boot_recovery::RecoveryEvent;

/// Options de fsck.
#[derive(Clone, Debug, Default)]
pub struct FsckOptions {
    pub repair:    bool,   // Si true, répare automatiquement.
    pub verbose:   bool,
    pub phase_mask: u8,    // Bitmask phases 1-4. 0x0F = toutes.
}

impl FsckOptions {
    pub fn full_repair() -> Self {
        Self { repair: true, verbose: false, phase_mask: 0x0F }
    }
    pub fn check_only() -> Self {
        Self { repair: false, verbose: false, phase_mask: 0x0F }
    }
}

/// Résultat d'une passe fsck.
#[derive(Clone, Debug, Default)]
pub struct PhaseResult {
    pub errors:   u32,
    pub warnings: u32,
    pub repaired: u32,
}

/// Résultat global du fsck.
#[derive(Clone, Debug, Default)]
pub struct FsckResult {
    pub phase1: PhaseResult,
    pub phase2: PhaseResult,
    pub phase3: PhaseResult,
    pub phase4: PhaseResult,
    pub total_errors:   u32,
    pub total_repaired: u32,
    pub clean:          bool,
}

impl FsckResult {
    pub fn total_errors(&self) -> u32 {
        self.phase1.errors + self.phase2.errors + self.phase3.errors + self.phase4.errors
    }
}

pub struct Fsck;

impl Fsck {
    pub fn run(device: &mut dyn BlockDevice, opts: &FsckOptions) -> Result<FsckResult, FsError> {
        RECOVERY_LOG.log_event(RecoveryEvent::FsckStarted);
        let mut result = FsckResult::default();

        // Phase 1 : Superblock.
        if opts.phase_mask & 0x01 != 0 {
            result.phase1 = Phase1Checker::run(device, opts.repair)?;
        }

        // Phase 2 : Heap scan.
        if opts.phase_mask & 0x02 != 0 {
            result.phase2 = Phase2Checker::run(device, opts.repair)?;
        }

        // Phase 3 : Reconstruction graphe.
        if opts.phase_mask & 0x04 != 0 {
            result.phase3 = Phase3Checker::run(device, opts.repair)?;
        }

        // Phase 4 : Détection orphelins.
        if opts.phase_mask & 0x08 != 0 {
            result.phase4 = Phase4Checker::run(device, opts.repair)?;
        }

        result.total_errors   = result.total_errors();
        result.total_repaired = result.phase1.repaired
                              + result.phase2.repaired
                              + result.phase3.repaired
                              + result.phase4.repaired;
        result.clean = result.total_errors == 0;

        RECOVERY_LOG.log_event(RecoveryEvent::FsckDone);
        if result.total_repaired > 0 {
            RECOVERY_LOG.log_event(RecoveryEvent::RepairApplied(result.total_repaired));
        }

        Ok(result)
    }
}
