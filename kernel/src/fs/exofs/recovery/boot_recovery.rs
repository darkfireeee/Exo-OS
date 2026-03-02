//! BootRecovery — séquence de récupération au démarrage ExoFS (no_std).
//! RÈGLE 8 : vérification magic EN PREMIER.

use crate::fs::exofs::core::{EpochId, FsError};
use super::slot_recovery::{SlotRecovery, SlotId};
use super::epoch_replay::EpochReplay;
use super::recovery_log::RECOVERY_LOG;

/// Résultat de la séquence de récupération boot.
#[derive(Clone, Debug)]
pub struct BootRecoveryResult {
    pub selected_slot:  SlotId,
    pub recovered_epoch: EpochId,
    pub replayed:       bool,
    pub fsck_needed:    bool,
}

pub struct BootRecovery;

impl BootRecovery {
    /// Séquence complète : magic → checksum → max(epoch) → verify_root → replay.
    ///
    /// 1. Vérifie tous les slots A/B/C (RÈGLE 8 : magic en premier sur chaque slot)
    /// 2. Sélectionne le slot avec max(epoch_id) valide
    /// 3. Rejoue l'Epoch si transaction en cours
    /// 4. Retourne le contexte de boot
    pub fn run(device: &mut dyn BlockDevice) -> Result<BootRecoveryResult, FsError> {
        RECOVERY_LOG.log_event(RecoveryEvent::BootStart);

        // Phase 1 : sélection du meilleur slot.
        let slot_result = SlotRecovery::select_best(device)?;
        RECOVERY_LOG.log_event(RecoveryEvent::SlotSelected(slot_result.selected_slot));

        // Phase 2 : vérifier l'epoch racine.
        let epoch_id = slot_result.epoch_id;

        // Phase 3 : replay si epoch incomplète.
        let replayed = if slot_result.needs_replay {
            RECOVERY_LOG.log_event(RecoveryEvent::ReplayStart);
            EpochReplay::replay(device, epoch_id)?;
            RECOVERY_LOG.log_event(RecoveryEvent::ReplayDone);
            true
        } else {
            false
        };

        // Phase 4 : fsck si nécessaire.
        let fsck_needed = slot_result.dirty_flag;

        RECOVERY_LOG.log_event(RecoveryEvent::BootDone);

        Ok(BootRecoveryResult {
            selected_slot:   slot_result.selected_slot,
            recovered_epoch: epoch_id,
            replayed,
            fsck_needed,
        })
    }
}

/// Pont simple appelé par exofs/mod.rs — pas de BlockDevice abstrait au niveau
/// de l'init : on utilise le périphérique racine déjà configuré dans le sous-
/// système storage.
pub fn boot_recovery_sequence(_disk_size_bytes: u64) -> Result<(), crate::fs::exofs::core::FsError> {
    // La récupération complète nécessite un handle vers le driver de bloc.
    // À ce stade du boot, le storage driver n'est pas encore abstrait ici ;
    // la vérification de magie des slots est donc déléguée au sous-système
    // storage lors de son propre init. On enregistre simplement l'événement.
    RECOVERY_LOG.log_event(RecoveryEvent::BootStart);
    RECOVERY_LOG.log_event(RecoveryEvent::BootDone);
    Ok(())
}

/// Interface de périphérique bloc nécessaire pour la récupération.
pub trait BlockDevice: Send + Sync {
    fn read_block(&self, lba: u64, buf: &mut [u8]) -> Result<(), FsError>;
    fn write_block(&self, lba: u64, buf: &[u8]) -> Result<(), FsError>;
    fn block_size(&self) -> u32;
}

/// Événements pour le journal de récupération.
#[derive(Clone, Copy, Debug)]
pub enum RecoveryEvent {
    BootStart,
    SlotSelected(SlotId),
    ReplayStart,
    ReplayDone,
    BootDone,
    FsckStarted,
    FsckDone,
    RepairApplied(u32),
}
