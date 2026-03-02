// kernel/src/fs/exofs/epoch/epoch_recovery.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Récupération de l'Epoch actif au montage
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Algorithme :
//   1. Lire les 3 slots (A, B, C) et valider magic + checksum (CHAIN-01).
//   2. Sélectionner le slot avec max(epoch_id) parmi les valides.
//   3. Vérifier l'intégrité de l'EpochRoot pointé par ce record (CHAIN-01).
//   4. Reconstruire le EpochSlotSelector pour les commits futurs.
//   5. Retourner le EpochId actif.
//
// Si seulement 1/3 slots valides → log + continue (RULE EPOCH-04).
// Si 0/3 slots valides → Err(ExofsError::NoValidEpoch) = nouveau volume.
//
// RÈGLE CHAIN-01 : magic + checksum par page AVANT lecture des entrées.

use alloc::vec::Vec;

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, EpochId, DiskOffset,
};
use crate::fs::exofs::epoch::epoch_slots::{
    EpochSlot, EpochSlotSelector, parse_slot_data,
};
use crate::fs::exofs::epoch::epoch_record::EpochRecord;
use crate::fs::exofs::epoch::epoch_root::verify_epoch_root_page;
use crate::fs::exofs::storage::superblock::ExoSuperblockInMemory;
use crate::fs::exofs::core::stats::EXOFS_STATS;

// ─────────────────────────────────────────────────────────────────────────────
// Résultat de la récupération
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat de la procédure de récupération de l'epoch actif.
#[derive(Debug)]
pub struct RecoveryResult {
    /// Epoch actif retrouvé.
    pub active_epoch_id: EpochId,
    /// Slot contenant l'EpochRecord actif.
    pub active_slot: EpochSlot,
    /// Nombre de slots sains (1–3).
    pub valid_slot_count: u8,
    /// Sélecteur ready-to-use pour les commits suivants.
    pub slot_selector: EpochSlotSelector,
    /// Vrai si l'epoch retrouvé avait le flag RECOVERING (crash précédent).
    pub needs_redo: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// Interface de lecture disque injectée (abstraction pour testabilité)
// ─────────────────────────────────────────────────────────────────────────────

/// Fonction de lecture d'un bloc de 104 octets à l'offset donné.
pub type ReadFn<'a> = &'a dyn Fn(DiskOffset, &mut [u8; 104]) -> ExofsResult<()>;

/// Fonction de lecture d'une page EpochRoot complète (taille variable).
pub type ReadPageFn<'a> = &'a dyn Fn(DiskOffset, &mut Vec<u8>, usize) -> ExofsResult<()>;

// ─────────────────────────────────────────────────────────────────────────────
// Procédure principale de récupération
// ─────────────────────────────────────────────────────────────────────────────

/// Récupère l'Epoch actif en lisant et validant les 3 slots.
///
/// # Paramètres
/// - `superblock` : superblock in-memory à mettre à jour.
/// - `disk_size`  : taille totale du volume (pour le slot C).
/// - `read_fn`    : lecture 104 octets depuis un offset disque.
/// - `read_page`  : lecture d'une page EpochRoot depuis un offset disque.
///
/// # Retour
/// - `Ok(RecoveryResult)` en cas de succès.
/// - `Err(ExofsError::NoValidEpoch)` si aucun epoch trouvé (volume neuf ou totalement corrompu).
pub fn recover_active_epoch(
    superblock:  &ExoSuperblockInMemory,
    disk_size:   DiskOffset,
    read_fn:     ReadFn<'_>,
    read_page:   ReadPageFn<'_>,
) -> ExofsResult<RecoveryResult> {
    let mut selector = EpochSlotSelector::new(disk_size);
    let mut records: [Option<EpochRecord>; 3] = [None, None, None];

    // ── Étape 1 : lecture et validation des 3 slots ─────────────────────────
    let slots = [EpochSlot::A, EpochSlot::B, EpochSlot::C];
    for &slot in &slots {
        let offset = slot.disk_offset(disk_size)?;
        let mut buf = [0u8; 104];
        match read_fn(offset, &mut buf) {
            Ok(()) => {}
            Err(_) => {
                // i/o error sur ce slot : marqué invalide, on continue.
                selector.update_slot(slot, false, 0);
                EXOFS_STATS.inc_recovery_slot_io_errors();
                continue;
            }
        }
        match parse_slot_data(&buf) {
            Ok(Some(record)) => {
                selector.update_slot(slot, true, record.epoch_id);
                records[slot as usize] = Some(record);
            }
            Ok(None) => {
                // Slot vide (volume neuf ou effacé).
                selector.update_slot(slot, false, 0);
            }
            Err(ExofsError::ChecksumMismatch) => {
                // RÈGLE CHAIN-01 : checksum invalide = slot corrompu.
                selector.update_slot(slot, false, 0);
                EXOFS_STATS.inc_recovery_checksum_errors();
            }
            Err(e) => return Err(e),
        }
    }

    // ── Étape 2 : sélection du slot actif ──────────────────────────────────
    let valid_count = selector.valid_count();
    if valid_count == 0 {
        // Aucun slot valide : volume neuf ou totalement corrompu.
        return Err(ExofsError::NoValidEpoch);
    }
    let (active_slot, active_epoch_raw) = selector
        .find_latest_valid_slot()
        .ok_or(ExofsError::NoValidEpoch)?;

    let active_record = records[active_slot as usize]
        .as_ref()
        .ok_or(ExofsError::CorruptedStructure)?;

    // ── Étape 3 : vérification de l'EpochRoot pointé (RULE CHAIN-01) ───────
    let root_offset = DiskOffset(active_record.root_offset);
    if root_offset.0 != 0 {
        // Page size de l'EpochRoot : on lit 4096 octets (une page standard).
        let mut page_buf: Vec<u8> = Vec::new();
        page_buf.try_reserve(4096).map_err(|_| ExofsError::NoMemory)?;
        page_buf.resize(4096, 0u8);
        let mut arr = [0u8; 104];
        let _ = arr; // kept for size check
        read_page(root_offset, &mut page_buf, 4096)?;
        // Vérification magic + checksum de la page EpochRoot.
        verify_epoch_root_page(&page_buf)?;
    }

    // ── Étape 4 : reconstruction du EpochId actif ───────────────────────────
    let active_epoch_id = EpochId(active_epoch_raw);
    let needs_redo = active_record.is_recovering();

    // ── Étape 5 : mise à jour du superblock in-memory ───────────────────────
    superblock.advance_epoch(active_epoch_id);

    // ── Log de récupération ─────────────────────────────────────────────────
    if valid_count < 3 {
        EXOFS_STATS.inc_recovery_degraded_mounts();
    }

    Ok(RecoveryResult {
        active_epoch_id,
        active_slot,
        valid_slot_count: valid_count,
        slot_selector:    selector,
        needs_redo,
    })
}
