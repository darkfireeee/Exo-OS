//! fsck.rs — Orchestrateur global du fsck ExoFS (phases 1 à 4).
//!
//! Exécute les quatre phases du contrôle de cohérence du système de fichiers :
//! - **Phase 1** : Validation du superbloc et des structures de base.
//! - **Phase 2** : Construction de la table de références blobs.
//! - **Phase 3** : Cohérence des chaînes de snapshots.
//! - **Phase 4** : Récupération des blobs orphelins.
//!
//! Un mode "repair" optionnel applique des actions correctives après chaque phase.
//!
//! # Règles spec appliquées
//! - **HDR-03** / **HASH-02** / **OOM-02** / **ARITH-02** : délégués aux phases.
//! - **WRITE-02** : délégué à `FsckRepair::apply`.
//! - **ONDISK-03** : pas d `AtomicU64` dans `repr(C)`.

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::fs::exofs::core::{ExofsError, ExofsResult};
use super::boot_recovery::BlockDevice;
use super::fsck_phase1::{FsckPhase1, Phase1Options, Phase1Report};
use super::fsck_phase2::{FsckPhase2, Phase2Options, Phase2Report};
use super::fsck_phase3::{FsckPhase3, Phase3Options, Phase3Report};
use super::fsck_phase4::{FsckPhase4, Phase4Options, Phase4Report};
use super::fsck_repair::{FsckRepair, RepairAction, REPAIR_LOG};
use super::checkpoint::{CHECKPOINT_STORE, RecoveryPhase};
use super::recovery_audit::RECOVERY_AUDIT;
use super::recovery_log::RECOVERY_LOG;

// ── Garde d exécution ─────────────────────────────────────────────────────────

/// Protège contre l exécution concurrente d un fsck.
static FSCK_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

// ── Options du fsck ───────────────────────────────────────────────────────────

/// Options globales de l exécution du fsck.
#[derive(Clone, Debug)]
pub struct FsckOptions {
    // ── Sélection des phases ──────────────────────────────────────────────────
    pub run_phase1:  bool,
    pub run_phase2:  bool,
    pub run_phase3:  bool,
    pub run_phase4:  bool,

    // ── Comportement en cas d erreur ──────────────────────────────────────────
    /// Si `true`, stoppe le fsck dès la première erreur critique.
    pub stop_on_critical: bool,
    /// Nombre maximal d erreurs cumulées avant abandon.
    pub max_total_errors: u32,

    // ── Réparation automatique ────────────────────────────────────────────────
    /// Si `true`, applique automatiquement les réparations après chaque phase.
    pub auto_repair:  bool,
    /// Si `true`, ne fait que simuler les réparations (aucune écriture).
    pub dry_run:      bool,

    // ── Options par phase ─────────────────────────────────────────────────────
    pub phase1_opts:  Phase1Options,
    pub phase2_opts:  Phase2Options,
    pub phase3_opts:  Phase3Options,
    pub phase4_opts:  Phase4Options,

    // ── Checkpointing ─────────────────────────────────────────────────────────
    /// Si `true`, sauvegarde un checkpoint après chaque phase.
    pub save_checkpoints: bool,
}

impl Default for FsckOptions {
    fn default() -> Self {
        Self {
            run_phase1:       true,
            run_phase2:       true,
            run_phase3:       true,
            run_phase4:       true,
            stop_on_critical: false,
            max_total_errors: 1024,
            auto_repair:      false,
            dry_run:          false,
            phase1_opts:      Phase1Options::default(),
            phase2_opts:      Phase2Options::default(),
            phase3_opts:      Phase3Options::default(),
            phase4_opts:      Phase4Options::default(),
            save_checkpoints: true,
        }
    }
}

impl FsckOptions {
    /// Crée des options pour un fsck rapide (phases 1+2 uniquement, sans réparation).
    pub fn quick() -> Self {
        Self {
            run_phase3:  false,
            run_phase4:  false,
            auto_repair: false,
            save_checkpoints: false,
            ..Self::default()
        }
    }

    /// Crée des options pour un fsck complet avec réparation.
    pub fn full_repair() -> Self {
        Self {
            auto_repair: true,
            dry_run: false,
            ..Self::default()
        }
    }

    /// Crée des options pour une simulation complète (dry-run).
    pub fn dry_run() -> Self {
        Self {
            auto_repair: true,
            dry_run: true,
            ..Self::default()
        }
    }

    /// Retourne le masque binaire des phases activées.
    ///
    /// bit0=phase1, bit1=phase2, bit2=phase3, bit3=phase4.
    pub fn phases_bitmask(&self) -> u8 {
        let mut mask = 0u8;
        if self.run_phase1 { mask |= 0x01; }
        if self.run_phase2 { mask |= 0x02; }
        if self.run_phase3 { mask |= 0x04; }
        if self.run_phase4 { mask |= 0x08; }
        mask
    }
}

// ── Résultat global du fsck ───────────────────────────────────────────────────

/// Résultat complet de l exécution du fsck.
#[derive(Clone, Debug)]
pub struct FsckResult {
    // ── Compteurs d erreurs par phase ─────────────────────────────────────────
    pub phase1_errors: u32,
    pub phase2_errors: u32,
    pub phase3_errors: u32,
    pub phase4_errors: u32,
    pub total_errors:  u32,

    // ── Compteurs de réparations ──────────────────────────────────────────────
    pub total_repairs:     u32,
    pub repairs_succeeded: u32,
    pub repairs_failed:    u32,

    // ── Phases exécutées (bitmask) ────────────────────────────────────────────
    /// bit0=phase1, bit1=phase2, bit2=phase3, bit3=phase4.
    pub phases_completed:  u8,

    // ── Statistiques ─────────────────────────────────────────────────────────
    pub blobs_checked:     u64,
    pub snapshots_checked: u64,
    pub orphans_recovered: u64,
    pub bytes_recovered:   u64,

    // ── Rapport de phase 1 ────────────────────────────────────────────────────
    pub phase1: Option<Phase1Report>,
    /// Rapport de phase 2 (contient la table de références).
    pub phase2: Option<Phase2Report>,
    pub phase3: Option<Phase3Report>,
    pub phase4: Option<Phase4Report>,

    pub dry_run: bool,
}

impl FsckResult {
    /// `true` si aucune erreur n a été détectée.
    pub fn is_clean(&self) -> bool { self.total_errors == 0 }
    /// `true` si toutes les phases demandées ont été exécutées.
    pub fn phases_all_done(&self, expected_mask: u8) -> bool {
        self.phases_completed & expected_mask == expected_mask
    }
    /// Taux de réparation (0..=100).
    pub fn repair_rate_pct(&self) -> u32 {
        if self.total_repairs == 0 { return 100; }
        self.repairs_succeeded
            .saturating_mul(100)
            .checked_div(self.total_repairs)
            .unwrap_or(0)
    }
}

// ── Orchestrateur global ──────────────────────────────────────────────────────

/// Orchestrateur global du fsck ExoFS.
pub struct Fsck;

impl Fsck {
    /// Exécute le fsck avec les options par défaut.
    pub fn run(device: &mut dyn BlockDevice) -> ExofsResult<FsckResult> {
        Self::run_with_options(device, &FsckOptions::default())
    }

    /// Exécute le fsck complet avec des options personnalisées.
    ///
    /// # Concurrence
    /// Retourne `ExofsError::CommitInProgress` si un fsck est déjà en cours.
    ///
    /// # Algorithm
    /// 1. Acquérir le verrou `FSCK_IN_PROGRESS`.
    /// 2. Phase 1 → valider superbloc.
    /// 3. Phase 2 → construire la table de références.
    /// 4. Phase 3 → vérifier les chaînes de snapshots.
    /// 5. Phase 4 → récupérer les blobs orphelins.
    /// 6. Sauvegarder un checkpoint final.
    /// 7. Libérer le verrou.
    pub fn run_with_options(
        device: &mut dyn BlockDevice,
        opts:   &FsckOptions,
    ) -> ExofsResult<FsckResult> {
        // Verrou d exclusion mutuelle.
        if FSCK_IN_PROGRESS.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            return Err(ExofsError::CommitInProgress);
        }

        let result = Self::run_inner(device, opts);

        FSCK_IN_PROGRESS.store(false, Ordering::SeqCst);
        result
    }

    fn run_inner(
        device: &mut dyn BlockDevice,
        opts:   &FsckOptions,
    ) -> ExofsResult<FsckResult> {
        RECOVERY_LOG.log_event(super::recovery_log::RecoveryEvent::FsckStarted);

        let mut res = FsckResult {
            phase1_errors: 0,
            phase2_errors: 0,
            phase3_errors: 0,
            phase4_errors: 0,
            total_errors:  0,
            total_repairs: 0,
            repairs_succeeded: 0,
            repairs_failed: 0,
            phases_completed: 0,
            blobs_checked:     0,
            snapshots_checked: 0,
            orphans_recovered: 0,
            bytes_recovered:   0,
            phase1: None,
            phase2: None,
            phase3: None,
            phase4: None,
            dry_run: opts.dry_run,
        };

        // ── Phase 1 ───────────────────────────────────────────────────────────
        if opts.run_phase1 {
            let p1 = FsckPhase1::run_with_options(device, &opts.phase1_opts)?;
            let ec = p1.errors.len() as u32;
            res.phase1_errors = ec;
            res.total_errors  = res.total_errors.saturating_add(ec);
            res.phases_completed |= 0x01;

            if opts.auto_repair && ec > 0 {
                let applied = Self::repair_phase1(device, &p1, opts.dry_run)?;
                res.total_repairs     = res.total_repairs.saturating_add(applied.0);
                res.repairs_succeeded = res.repairs_succeeded.saturating_add(applied.1);
                res.repairs_failed    = res.repairs_failed.saturating_add(applied.0.saturating_sub(applied.1));
            }

            if opts.save_checkpoints {
                let _ = CHECKPOINT_STORE.save_phase(1, RecoveryPhase::Phase1Done);
            }

            if opts.stop_on_critical && p1.has_critical_errors() {
                res.phase1 = Some(p1);
                RECOVERY_LOG.log_event(super::recovery_log::RecoveryEvent::FsckDone);
                return Ok(res);
            }
            res.phase1 = Some(p1);
        }

        if res.total_errors >= opts.max_total_errors { return Ok(res); }

        // ── Phase 2 ───────────────────────────────────────────────────────────
        if opts.run_phase2 {
            let p2 = FsckPhase2::run_with_options(device, &opts.phase2_opts)?;
            let ec = p2.errors.len() as u32;
            res.phase2_errors = ec;
            res.total_errors  = res.total_errors.saturating_add(ec);
            res.blobs_checked = p2.blobs_walked;
            res.phases_completed |= 0x02;

            if opts.save_checkpoints {
                let _ = CHECKPOINT_STORE.save_phase(2, RecoveryPhase::Phase2Done);
            }

            if opts.stop_on_critical && ec > 0 && p2.has_critical_errors() {
                res.phase2 = Some(p2);
                return Ok(res);
            }
            res.phase2 = Some(p2);
        }

        if res.total_errors >= opts.max_total_errors { return Ok(res); }

        // ── Phase 3 ───────────────────────────────────────────────────────────
        if opts.run_phase3 {
            let ref_counter = match res.phase2.as_ref() {
                Some(p2) => &p2.ref_counter,
                None => {
                    // Phase 2 non exécutée : on saute la phase 3.
                    RECOVERY_LOG.log_event(super::recovery_log::RecoveryEvent::FsckDone);
                    res.phases_completed |= 0x04;
                    // Passer directement à la phase 4 avec un rapport vide.
                    let p3 = Phase3Report {
                        errors:            alloc::vec![],
                        snapshots_checked: 0,
                        snapshots_ok:      0,
                        orphan_snapshots:  0,
                        chains_ok:         0,
                        cycle_count:       0,
                        critical_errors:   0,
                        deleted_skipped:   0,
                    };
                    res.phase3 = Some(p3);
                    return Ok(res);
                }
            };

            let p3 = FsckPhase3::run_with_options(device, ref_counter, &opts.phase3_opts)?;
            let ec = p3.errors.len() as u32;
            res.phase3_errors     = ec;
            res.total_errors      = res.total_errors.saturating_add(ec);
            res.snapshots_checked = p3.snapshots_checked;
            res.phases_completed |= 0x04;

            if opts.save_checkpoints {
                let _ = CHECKPOINT_STORE.save_phase(3, RecoveryPhase::Phase3Done);
            }

            if opts.stop_on_critical && p3.has_criticals() {
                res.phase3 = Some(p3);
                return Ok(res);
            }
            res.phase3 = Some(p3);
        }

        if res.total_errors >= opts.max_total_errors { return Ok(res); }

        // ── Phase 4 ───────────────────────────────────────────────────────────
        if opts.run_phase4 {
            let p2 = match res.phase2.as_ref() {
                Some(p) => p,
                None => {
                    res.phases_completed |= 0x08;
                    RECOVERY_LOG.log_event(super::recovery_log::RecoveryEvent::FsckDone);
                    return Ok(res);
                }
            };

            let mut p4_opts = opts.phase4_opts;
            p4_opts.dry_run = opts.dry_run;

            let p4 = FsckPhase4::run_with_options(device, p2, &p4_opts)?;
            let ec = p4.errors.len() as u32;
            res.phase4_errors     = ec;
            res.total_errors      = res.total_errors.saturating_add(ec);
            res.orphans_recovered = p4.orphans_recovered;
            res.bytes_recovered   = p4.bytes_recovered;
            res.phases_completed |= 0x08;

            if opts.save_checkpoints {
                let _ = CHECKPOINT_STORE.save_phase(4, RecoveryPhase::Phase4Done);
            }

            res.phase4 = Some(p4);
        }

        RECOVERY_LOG.log_fsck_done(res.total_errors);
        RECOVERY_LOG.log_event(super::recovery_log::RecoveryEvent::FsckDone);

        Ok(res)
    }

    // ── Réparation post-phase 1 ───────────────────────────────────────────────

    /// Génère et applique les actions de réparation suite à la phase 1.
    ///
    /// Retourne `(total_applied, succeeded)`.
    fn repair_phase1(
        device:  &mut dyn BlockDevice,
        p1:      &Phase1Report,
        dry_run: bool,
    ) -> ExofsResult<(u32, u32)> {
        let mut total     = 0u32;
        let mut succeeded = 0u32;

        // Si le superbloc est corrompu, tenter une reconstruction.
        if p1.superblock_corrupt {
            let action = RepairAction::WriteFallbackSuperblock {
                lba: super::fsck_phase1::SUPERBLOCK_LBA,
            };
            total = total.saturating_add(1);
            match FsckRepair::apply(device, action, dry_run) {
                Ok(_) => { succeeded = succeeded.saturating_add(1); }
                Err(_) => {}
            }
        }

        // Pour chaque erreur dans p1.errors, tenter une réparation ciblée.
        for error in &p1.errors {
            use super::fsck_phase1::Phase1ErrorKind;
            let action_opt = match error.kind {
                Phase1ErrorKind::SuperblockBadMagic
                | Phase1ErrorKind::SuperblockBadChecksum => Some(
                    RepairAction::WriteFallbackSuperblock {
                        lba: super::fsck_phase1::SUPERBLOCK_LBA,
                    }
                ),
                _ => None,
            };
            if let Some(action) = action_opt {
                total = total.saturating_add(1);
                match FsckRepair::apply(device, action, dry_run) {
                    Ok(_) => { succeeded = succeeded.saturating_add(1); }
                    Err(_) => {}
                }
            }
        }

        Ok((total, succeeded))
    }
}

// ── Extensions de rapport ─────────────────────────────────────────────────────

impl Phase1Report {
    /// Retourne `true` si des erreurs critiques ont été détectées en phase 1.
    pub fn has_critical_errors(&self) -> bool {
        use super::fsck_phase1::Phase1ErrorKind;
        self.errors.iter().any(|e| matches!(
            e.kind,
            Phase1ErrorKind::SuperblockBadMagic
            | Phase1ErrorKind::SuperblockBadChecksum
        ))
    }
}

impl Phase2Report {
    /// Retourne `true` si des erreurs critiques ont été détectées en phase 2.
    pub fn has_critical_errors(&self) -> bool {
        use super::fsck_phase2::Phase2ErrorKind;
        self.errors.iter().any(|e| matches!(
            e.kind,
            Phase2ErrorKind::HeaderBadMagic
            | Phase2ErrorKind::HeaderBadChecksum
            | Phase2ErrorKind::DataHashMismatch
        ))
    }
}

// ── CheckpointStore extensions ────────────────────────────────────────────────

impl super::checkpoint::CheckpointStore {
    /// Sauvegarde un checkpoint à la phase donnée.
    pub fn save_phase(&self, _phase_num: u8, phase: RecoveryPhase) -> ExofsResult<()> {
        let tick = crate::arch::time::read_ticks();
        let _ = self.save_checkpoint(tick, phase, 0);
        Ok(())
    }
}

// ── Tests unitaires ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fsck_options_default() {
        let opts = FsckOptions::default();
        assert!(opts.run_phase1);
        assert!(opts.run_phase2);
        assert!(opts.run_phase3);
        assert!(opts.run_phase4);
        assert!(!opts.dry_run);
        assert!(!opts.auto_repair);
    }

    #[test]
    fn test_fsck_options_bitmask() {
        let opts = FsckOptions::default();
        assert_eq!(opts.phases_bitmask(), 0x0F);
        let quick = FsckOptions::quick();
        assert_eq!(quick.phases_bitmask(), 0x03); // phases 1+2.
    }

    #[test]
    fn test_fsck_result_clean() {
        let r = FsckResult {
            phase1_errors: 0,
            phase2_errors: 0,
            phase3_errors: 0,
            phase4_errors: 0,
            total_errors:  0,
            total_repairs: 0,
            repairs_succeeded: 0,
            repairs_failed: 0,
            phases_completed: 0x0F,
            blobs_checked:     100,
            snapshots_checked: 10,
            orphans_recovered: 0,
            bytes_recovered:   0,
            phase1: None,
            phase2: None,
            phase3: None,
            phase4: None,
            dry_run: false,
        };
        assert!(r.is_clean());
        assert!(r.phases_all_done(0x0F));
        assert_eq!(r.repair_rate_pct(), 100);
    }

    #[test]
    fn test_fsck_result_repair_rate() {
        let r = FsckResult {
            phase1_errors: 2,
            phase2_errors: 0,
            phase3_errors: 0,
            phase4_errors: 0,
            total_errors:  2,
            total_repairs: 4,
            repairs_succeeded: 3,
            repairs_failed: 1,
            phases_completed: 0x0F,
            blobs_checked:     50,
            snapshots_checked: 5,
            orphans_recovered: 2,
            bytes_recovered:   1024,
            phase1: None,
            phase2: None,
            phase3: None,
            phase4: None,
            dry_run: false,
        };
        assert_eq!(r.repair_rate_pct(), 75);
    }

    #[test]
    fn test_fsck_options_quick() {
        let opts = FsckOptions::quick();
        assert!(opts.run_phase1);
        assert!(opts.run_phase2);
        assert!(!opts.run_phase3);
        assert!(!opts.run_phase4);
    }

    #[test]
    fn test_fsck_options_dry_run() {
        let opts = FsckOptions::dry_run();
        assert!(opts.dry_run);
        assert!(opts.auto_repair);
    }
}
