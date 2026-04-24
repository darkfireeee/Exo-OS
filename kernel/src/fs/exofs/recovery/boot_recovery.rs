//! boot_recovery.rs — Séquence de récupération au démarrage ExoFS (no_std).
//!
//! Orchestre la récupération complète lors du montage du système de fichiers :
//! sélection du meilleur slot A/B/C, replay de l'epoch incomplète, fsck conditionnel.
//!
//! # Règles spec appliquées
//! - **HDR-03** : magic vérifié EN PREMIER sur tous les en-têtes (délégué aux sous-modules).
//! - **HASH-02** : `verify_blob_id` sur les données rejouées (délégué à `epoch_replay`).
//! - **OOM-02** : `try_reserve(1)` avant tout `Vec::push`.
//! - **ARITH-02** : `checked_add` pour les compteurs.
//! - **WRITE-02** : vérification des écritures dans les sous-modules.

extern crate alloc;
use super::checkpoint::{RecoveryPhase, CHECKPOINT_STORE};
use super::fsck::FsckOptions;
use super::recovery_audit::RECOVERY_AUDIT;
use super::recovery_log::RECOVERY_LOG;
use crate::fs::exofs::core::{EpochId, ExofsError, ExofsResult};
use core::sync::atomic::{AtomicBool, Ordering};

// ── Trait périphérique bloc ───────────────────────────────────────────────────

/// Abstraction d'un périphérique bloc pour les opérations de récupération.
///
/// Implémenté par le sous-système storage pour passer le handle device
/// aux modules de récupération (slot_recovery, epoch_replay, fsck, …).
pub trait BlockDevice: Send + Sync {
    /// Lit un bloc de taille `device.block_size()` depuis le LBA donné.
    fn read_block(&self, lba: u64, buf: &mut [u8]) -> ExofsResult<()>;

    /// Écrit un bloc de taille `device.block_size()` au LBA donné.
    ///
    /// # WRITE-02
    /// Les implémentations doivent garantir que `buf.len() == block_size()`.
    fn write_block(&self, lba: u64, buf: &[u8]) -> ExofsResult<()>;

    /// Retourne la taille d'un bloc en octets (ex. 512, 4096).
    fn block_size(&self) -> u32;

    /// Retourne la taille totale du périphérique en blocs.
    fn total_blocks(&self) -> u64;

    /// Flush / barrière mémoire NVMe (équivalent FLUSH+FUA).
    ///
    /// Doit être appelé entre les phases data → root → record (RÈGLE 7).
    fn flush(&self) -> ExofsResult<()>;
}

// ── Options de récupération ───────────────────────────────────────────────────

/// Options configurant la séquence de boot recovery.
#[derive(Clone, Copy, Debug)]
pub struct BootRecoveryOptions {
    /// Forcer le fsck même si le dirty flag est absent.
    pub force_fsck: bool,
    /// Mode dry-run : aucune écriture sur disque.
    pub dry_run: bool,
    /// Nombre maximal d'erreurs avant abandon du fsck.
    pub max_fsck_errors: u32,
    /// Activer le replay d'epoch même si `dirty_flag` est faux.
    pub force_replay: bool,
    /// Timeout de récupération en millisecondes (0 = pas de timeout).
    pub timeout_ms: u64,
}

impl Default for BootRecoveryOptions {
    fn default() -> Self {
        Self {
            force_fsck: false,
            dry_run: false,
            max_fsck_errors: 256,
            force_replay: false,
            timeout_ms: 0,
        }
    }
}

// ── Résultat de récupération ──────────────────────────────────────────────────

/// Résultat de la séquence complète de boot recovery.
#[derive(Clone, Debug)]
pub struct BootRecoveryResult {
    /// Slot sélectionné (0=A, 1=B, 2=C).
    pub selected_slot: u8,
    /// EpochId récupérée depuis le slot sélectionné.
    pub recovered_epoch: EpochId,
    /// `true` si un replay d'epoch a été effectué.
    pub replayed: bool,
    /// Nombre de blocs rejoués.
    pub n_replayed: u32,
    /// `true` si un fsck a été requis.
    pub fsck_needed: bool,
    /// `true` si un fsck a été effectivement exécuté.
    pub fsck_done: bool,
    /// Nombre total d'erreurs détectées.
    pub total_errors: u32,
    /// Nombre total de réparations appliquées.
    pub total_repairs: u32,
    /// Phases fsck complétées (bitmask : bit N = phase N+1 terminée).
    pub phases_completed: u8,
    /// ID du checkpoint final sauvegardé.
    pub final_checkpoint_id: u64,
}

impl BootRecoveryResult {
    /// `true` si la récupération s'est terminée sans erreur.
    #[inline]
    pub fn is_clean(&self) -> bool {
        self.total_errors == 0
    }

    /// `true` si des réparations ont été nécessaires.
    #[inline]
    pub fn had_repairs(&self) -> bool {
        self.total_repairs > 0
    }
}

// ── Étapes internes ───────────────────────────────────────────────────────────

/// Avancement interne de la séquence boot recovery (pour checkpoints progressifs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
enum BootStep {
    Init,
    SlotSelected,
    EpochVerified,
    ReplayDone,
    FsckDone,
    Complete,
}

// ── Récupération en cours (guard atomique) ─────────────────────────────────────

/// `true` si une séquence de boot recovery est en cours.
static RECOVERY_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

// ── Implémentation principale ─────────────────────────────────────────────────

/// Exécuteur de la séquence de boot recovery.
pub struct BootRecovery;

impl BootRecovery {
    /// Exécute la séquence complète de boot recovery.
    ///
    /// # Séquence
    /// 1. Log `BootStart` + audit `RecoveryStarted`.
    /// 2. Lire et valider les 3 slots (HDR-03 dans `slot_recovery`).
    /// 3. Sélectionner le slot avec `max(epoch_id)` valide.
    /// 4. Si `dirty_flag` || `force_replay` → replay epoch (HASH-02 dans `epoch_replay`).
    /// 5. Sauvegarder un checkpoint intermédiaire.
    /// 6. Si `dirty_flag` || `force_fsck` → déclencher fsck (phases 1→4).
    /// 7. Sauvegarder un checkpoint final.
    /// 8. Log `BootDone` + audit `RecoveryCompleted`.
    ///
    /// # Erreurs
    /// - `ExofsError::InvalidState` : une récupération est déjà en cours.
    /// - Toute erreur remontée par `slot_recovery`, `epoch_replay` ou `fsck`.
    pub fn run(
        device: &mut dyn BlockDevice,
        options: &BootRecoveryOptions,
    ) -> ExofsResult<BootRecoveryResult> {
        // Guard : une seule récupération à la fois.
        if RECOVERY_IN_PROGRESS.swap(true, Ordering::SeqCst) {
            return Err(ExofsError::CommitInProgress);
        }

        let result = Self::run_inner(device, options);

        RECOVERY_IN_PROGRESS.store(false, Ordering::SeqCst);
        result
    }

    fn run_inner(
        device: &mut dyn BlockDevice,
        options: &BootRecoveryOptions,
    ) -> ExofsResult<BootRecoveryResult> {
        // ── Initialisation ──────────────────────────────────────────────────
        RECOVERY_LOG.log_boot_start();
        RECOVERY_AUDIT.record_recovery_started(EpochId(0));

        let mut total_errors: u32 = 0;
        let mut total_repairs: u32 = 0;
        let mut phases_completed: u8 = 0;

        // ── Étape 1 : sélection du slot ────────────────────────────────────
        let slot_result = super::slot_recovery::SlotRecovery::select_best(device).map_err(|e| {
            RECOVERY_LOG.log_error(0x01, 0);
            RECOVERY_AUDIT.record_recovery_failed(EpochId(0), e as u32);
            e
        })?;

        RECOVERY_LOG.log_slot_selected(slot_result.selected_slot.0);
        RECOVERY_AUDIT.record_slot_selected(slot_result.selected_slot.0, slot_result.epoch_id.0);

        // Checkpoint après sélection de slot.
        let _cp1 = CHECKPOINT_STORE.save(
            RecoveryPhase::SlotRead,
            slot_result.epoch_id,
            total_errors,
            total_repairs,
        );

        // ── Étape 2 : vérification epoch ───────────────────────────────────
        let epoch_id = slot_result.epoch_id;

        let _cp2 = CHECKPOINT_STORE.save(
            RecoveryPhase::EpochFound,
            epoch_id,
            total_errors,
            total_repairs,
        );

        // ── Étape 3 : replay si nécessaire ─────────────────────────────────
        let needs_replay = slot_result.needs_replay || options.force_replay;
        let mut n_replayed = 0u32;
        let mut replayed = false;

        if needs_replay {
            RECOVERY_LOG.log_replay_start(epoch_id.0);

            let replay_result = super::epoch_replay::EpochReplay::replay(device, epoch_id)
                .map_err(|e| {
                    RECOVERY_LOG.log_error(0x02, epoch_id.0);
                    e
                })?;

            n_replayed = replay_result.n_replayed;
            replayed = true;

            RECOVERY_LOG.log_replay_done(n_replayed);
            RECOVERY_AUDIT.record_epoch_replayed(epoch_id, n_replayed);

            if replay_result.n_skipped > 0 {
                // ARITH-02 : checked_add pour le compteur d'erreurs.
                total_errors = total_errors
                    .checked_add(replay_result.n_skipped)
                    .unwrap_or(u32::MAX);
            }

            let _cp3 = CHECKPOINT_STORE.save(
                RecoveryPhase::Replayed,
                epoch_id,
                total_errors,
                total_repairs,
            );
        }

        // ── Étape 4 : fsck conditionnel ────────────────────────────────────
        let fsck_needed = slot_result.dirty_flag || options.force_fsck;
        let mut fsck_done = false;

        if fsck_needed {
            RECOVERY_LOG.log_fsck_started();
            RECOVERY_AUDIT.record_phase_started(0); // Phase globale.

            let fsck_opts = super::fsck::FsckOptions {
                run_phase1: true,
                run_phase2: true,
                run_phase3: true,
                run_phase4: true,
                auto_repair: !options.dry_run,
                max_total_errors: options.max_fsck_errors,
                ..FsckOptions::default()
            };

            let fsck_result =
                super::fsck::Fsck::run_with_options(device, &fsck_opts).map_err(|e| {
                    RECOVERY_LOG.log_error(0x03, 0);
                    e
                })?;

            // ARITH-02 : checked_add pour cumuler les compteurs.
            total_errors = total_errors
                .checked_add(fsck_result.total_errors)
                .unwrap_or(u32::MAX);
            total_repairs = total_repairs
                .checked_add(fsck_result.total_repairs)
                .unwrap_or(u32::MAX);

            phases_completed = fsck_result.phases_completed;
            fsck_done = true;

            RECOVERY_LOG.log_fsck_done(total_errors);
            RECOVERY_AUDIT.record_phase_done(0, total_errors);

            // Checkpoints après chaque phase.
            if fsck_result.phases_completed & 0x01 != 0 {
                let _ = CHECKPOINT_STORE.save(
                    RecoveryPhase::Phase1Done,
                    epoch_id,
                    total_errors,
                    total_repairs,
                );
            }
            if fsck_result.phases_completed & 0x02 != 0 {
                let _ = CHECKPOINT_STORE.save(
                    RecoveryPhase::Phase2Done,
                    epoch_id,
                    total_errors,
                    total_repairs,
                );
            }
            if fsck_result.phases_completed & 0x04 != 0 {
                let _ = CHECKPOINT_STORE.save(
                    RecoveryPhase::Phase3Done,
                    epoch_id,
                    total_errors,
                    total_repairs,
                );
            }
            if fsck_result.phases_completed & 0x08 != 0 {
                let _ = CHECKPOINT_STORE.save(
                    RecoveryPhase::Phase4Done,
                    epoch_id,
                    total_errors,
                    total_repairs,
                );
            }
        }

        // ── Étape 5 : checkpoint final ─────────────────────────────────────
        let final_cp_id = CHECKPOINT_STORE
            .save(
                RecoveryPhase::Complete,
                epoch_id,
                total_errors,
                total_repairs,
            )
            .map(|id| id.0)
            .unwrap_or(0);

        RECOVERY_LOG.log_checkpoint_saved(final_cp_id);
        RECOVERY_LOG.log_boot_done();
        RECOVERY_AUDIT.record_recovery_completed(epoch_id, total_repairs);

        Ok(BootRecoveryResult {
            selected_slot: slot_result.selected_slot.0,
            recovered_epoch: epoch_id,
            replayed,
            n_replayed,
            fsck_needed,
            fsck_done,
            total_errors,
            total_repairs,
            phases_completed,
            final_checkpoint_id: final_cp_id,
        })
    }

    /// Retourne `true` si une récupération est actuellement en cours.
    #[inline]
    pub fn is_running() -> bool {
        RECOVERY_IN_PROGRESS.load(Ordering::SeqCst)
    }
}

// ── Point d'entrée simplifié (init ExoFS) ─────────────────────────────────────

/// Point d'entrée simplifié appellé par `exofs/mod.rs` lors de l'init.
///
/// Sans handle device concret à ce stade, enregistre les événements boot
/// minimaux et retourne `Ok(())`.
pub fn boot_recovery_sequence(_disk_size_bytes: u64) -> ExofsResult<()> {
    RECOVERY_LOG.log_boot_start();
    RECOVERY_AUDIT.record_recovery_started(EpochId(0));
    // Le fsck complet nécessite un handle BlockDevice fourni par le storage
    // driver. Voir BootRecovery::run() pour la séquence complète.
    RECOVERY_LOG.log_boot_done();
    RECOVERY_AUDIT.record_recovery_completed(EpochId(0), 0);
    Ok(())
}

// ── Rapport de santé ──────────────────────────────────────────────────────────

/// Rapport de santé du sous-système de boot recovery.
#[derive(Clone, Copy, Debug)]
pub struct BootRecoveryHealth {
    /// `true` si aucune récupération n'est en cours.
    pub idle: bool,
    /// Nombre de checkpoints en mémoire.
    pub checkpoint_count: usize,
    /// Phase la plus avancée vue.
    pub furthest_phase: RecoveryPhase,
    /// Erreurs dans le journal de récupération.
    pub log_error_count: usize,
    /// Violations d'intégrité dans l'audit.
    pub audit_violation_count: usize,
}

impl BootRecovery {
    /// Construit un rapport de santé du sous-système.
    pub fn health() -> BootRecoveryHealth {
        let cp_diag = CHECKPOINT_STORE.diagnostic();
        let log_diag = RECOVERY_LOG.diagnostic();
        let aud_diag = RECOVERY_AUDIT.diagnostic();
        BootRecoveryHealth {
            idle: !Self::is_running(),
            checkpoint_count: cp_diag.count,
            furthest_phase: cp_diag.latest_phase,
            log_error_count: log_diag.error_count,
            audit_violation_count: aud_diag.violations,
        }
    }
}

// ── Tests unitaires ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_options_default() {
        let opts = BootRecoveryOptions::default();
        assert!(!opts.force_fsck);
        assert!(!opts.dry_run);
        assert_eq!(opts.max_fsck_errors, 256);
    }

    #[test]
    fn test_boot_recovery_result_clean() {
        let r = BootRecoveryResult {
            selected_slot: 0,
            recovered_epoch: EpochId(1),
            replayed: false,
            n_replayed: 0,
            fsck_needed: false,
            fsck_done: false,
            total_errors: 0,
            total_repairs: 0,
            phases_completed: 0,
            final_checkpoint_id: 1,
        };
        assert!(r.is_clean());
        assert!(!r.had_repairs());
    }

    #[test]
    fn test_guard_not_running() {
        assert!(!BootRecovery::is_running());
    }

    #[test]
    fn test_boot_recovery_sequence() {
        let r = boot_recovery_sequence(1024 * 1024);
        assert!(r.is_ok());
    }
}
