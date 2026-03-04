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
//   5. Optionnellement rejouer le redo log si RECOVERING flag présent.
//   6. Retourner le EpochId actif + diagnostics.
//
// Si seulement 1/3 slots valides → mode dégradé (RULE EPOCH-04).
// Si 0/3 slots valides → Err(ExofsError::NoValidEpoch) = volume neuf.
//
// RÈGLE CHAIN-01 : magic + checksum par page AVANT lecture des entrées.
// RÈGLE DAG-01   : pas d'import storage/ — callbacks injectés.
// RÈGLE ARITH-02 : checked_add / saturating_* pour toute arithmétique.
// RÈGLE OOM-02   : try_reserve avant push.

use alloc::vec::Vec;
use core::fmt;

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, EpochId, DiskOffset,
};
use crate::fs::exofs::epoch::epoch_slots::{
    EpochSlot, EpochSlotSelector, parse_slot_data,
};
use crate::fs::exofs::epoch::epoch_record::EpochRecord;
use crate::fs::exofs::epoch::epoch_root::verify_epoch_root_page;
use crate::fs::exofs::epoch::epoch_stats::EPOCH_STATS;

// =============================================================================
// Phase de récupération
// =============================================================================

/// Phase en cours dans la procédure de récupération epoch.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RecoveryPhase {
    /// Initialisation — aucune phase lancée.
    Idle,
    /// Lecture des slots A/B/C.
    ReadingSlots,
    /// Sélection du slot actif parmi les valides.
    SelectingActiveSlot,
    /// Vérification de l'EpochRoot pointé par le record actif.
    VerifyingEpochRoot,
    /// Relecture du redo log (flag RECOVERING présent).
    ReplayingRedoLog,
    /// Mise à jour du superblock in-memory.
    UpdatingSuperblock,
    /// Récupération terminée avec succès.
    Complete,
    /// Récupération échouée.
    Failed,
}

impl fmt::Display for RecoveryPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecoveryPhase::Idle               => write!(f, "Idle"),
            RecoveryPhase::ReadingSlots       => write!(f, "ReadingSlots"),
            RecoveryPhase::SelectingActiveSlot => write!(f, "SelectingActiveSlot"),
            RecoveryPhase::VerifyingEpochRoot => write!(f, "VerifyingEpochRoot"),
            RecoveryPhase::ReplayingRedoLog   => write!(f, "ReplayingRedoLog"),
            RecoveryPhase::UpdatingSuperblock => write!(f, "UpdatingSuperblock"),
            RecoveryPhase::Complete           => write!(f, "Complete"),
            RecoveryPhase::Failed             => write!(f, "Failed"),
        }
    }
}

// =============================================================================
// Résultat de récupération
// =============================================================================

/// Résultat de la procédure de récupération de l'epoch actif.
#[derive(Debug)]
pub struct RecoveryResult {
    /// Epoch actif retrouvé.
    pub active_epoch_id:  EpochId,
    /// Slot contenant l'EpochRecord actif.
    pub active_slot:      EpochSlot,
    /// Nombre de slots sains (0–3).
    pub valid_slot_count: u8,
    /// Sélecteur prêt à l'emploi pour les commits suivants.
    pub slot_selector:    EpochSlotSelector,
    /// Vrai si l'epoch retrouvé avait le flag RECOVERING (crash précédent).
    pub needs_redo:       bool,
    /// Diagnostics détaillés de la récupération.
    pub diagnostics:      RecoveryDiagnostics,
}

// =============================================================================
// Diagnostics de récupération
// =============================================================================

/// Résultat de vérification pour un slot individuel.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SlotCheckResult {
    /// Slot valide avec l'epoch_id lu.
    Valid(u64),
    /// Slot vide (volume neuf ou blank).
    Empty,
    /// Erreur I/O lors de la lecture.
    IoError,
    /// Checksum invalide.
    ChecksumMismatch,
    /// Magic invalide.
    MagicMismatch,
    /// Slot non encore vérifié.
    NotChecked,
}

impl fmt::Display for SlotCheckResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SlotCheckResult::Valid(e)         => write!(f, "Valid(epoch={})", e),
            SlotCheckResult::Empty            => write!(f, "Empty"),
            SlotCheckResult::IoError          => write!(f, "IoError"),
            SlotCheckResult::ChecksumMismatch => write!(f, "ChecksumMismatch"),
            SlotCheckResult::MagicMismatch    => write!(f, "MagicMismatch"),
            SlotCheckResult::NotChecked       => write!(f, "NotChecked"),
        }
    }
}

/// Diagnostics complets de la procédure de récupération.
#[derive(Debug)]
pub struct RecoveryDiagnostics {
    /// Résultat par slot : [A, B, C].
    pub slot_results:         [SlotCheckResult; 3],
    /// Vrai si l'EpochRoot du record actif est valide.
    pub epoch_root_valid:     bool,
    /// Nombre d'epochs rejoués depuis le redo log (0 si pas de crash).
    pub redo_epochs_replayed: u32,
    /// Vrai si montage en mode dégradé (< 3 slots valides).
    pub degraded_mode:        bool,
    /// Phase finale atteinte.
    pub final_phase:          RecoveryPhase,
    /// Durée totale de récupération en cycles TSC.
    pub duration_cycles:      u64,
}

impl RecoveryDiagnostics {
    /// Crée des diagnostics vierges.
    fn new() -> Self {
        Self {
            slot_results:         [SlotCheckResult::NotChecked; 3],
            epoch_root_valid:     false,
            redo_epochs_replayed: 0,
            degraded_mode:        false,
            final_phase:          RecoveryPhase::Idle,
            duration_cycles:      0,
        }
    }
}

impl fmt::Display for RecoveryDiagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "RecoveryDiagnostics {{ A={}, B={}, C={}, root_valid={}, \
             redo_replayed={}, degraded={}, phase={}, cycles={} }}",
            self.slot_results[0],
            self.slot_results[1],
            self.slot_results[2],
            self.epoch_root_valid,
            self.redo_epochs_replayed,
            self.degraded_mode,
            self.final_phase,
            self.duration_cycles,
        )
    }
}

// =============================================================================
// Callbacks injectés (DAG-01 : pas d'import storage/)
// =============================================================================

/// Fonction de lecture d'un bloc de 104 octets à l'offset donné.
/// Signature : (offset, buf) → Ok(()) ou Err.
pub type ReadFn<'a> = &'a dyn Fn(DiskOffset, &mut [u8; 104]) -> ExofsResult<()>;

/// Fonction de lecture d'une page EpochRoot complète.
/// Signature : (offset, buf, max_len) → Ok(()) ou Err.
pub type ReadPageFn<'a> = &'a dyn Fn(DiskOffset, &mut Vec<u8>, usize) -> ExofsResult<()>;

/// Callback de mise à jour de l'EpochId courant (injecté par storage layer).
/// Signature : (new_epoch_id) → Ok(()) ou Err.
pub type SetEpochFn<'a> = &'a dyn Fn(EpochId) -> ExofsResult<()>;

/// Horloge TSC injectée.
pub type TscFn<'a> = &'a dyn Fn() -> u64;

// =============================================================================
// Paramètres de récupération
// =============================================================================

/// Paramètres regroupés pour la procédure de récupération.
pub struct RecoveryParams<'a> {
    /// Taille totale du volume en octets (pour le calcul de l'offset slot C).
    pub disk_size:    DiskOffset,
    /// Fonction de lecture 104 octets.
    pub read_fn:      ReadFn<'a>,
    /// Fonction de lecture d'une page (EpochRoot).
    pub read_page:    ReadPageFn<'a>,
    /// Callback pour mettre à jour l'epoch courant dans le superblock.
    pub set_epoch_fn: SetEpochFn<'a>,
    /// Timestamp TSC (pour mesure de durée).
    pub tsc_fn:       TscFn<'a>,
    /// Taille d'une page EpochRoot (généralement 4096).
    pub page_size:    usize,
}

// =============================================================================
// Procédure principale de récupération
// =============================================================================

/// Récupère l'Epoch actif en lisant et validant les 3 slots.
///
/// # Paramètres
/// - `params` : tous les paramètres regroupés (callbacks injectés, DAG-01).
///
/// # Retour
/// - `Ok(RecoveryResult)` en cas de succès.
/// - `Err(ExofsError::NoValidEpoch)` si aucun epoch valide (volume neuf).
/// - Autres erreurs : I/O failure critique.
///
/// # Invariants
/// - Magic TOUJOURS vérifié avant checksum (CHAIN-01).
/// - Aucune acquisition de EPOCH_COMMIT_LOCK ici (DEAD-01).
/// - Pas d'allocation infinie — tout Vec est pré-réservé (OOM-02).
pub fn recover_active_epoch(params: RecoveryParams<'_>) -> ExofsResult<RecoveryResult> {
    let start_tsc = (params.tsc_fn)();
    let mut diag  = RecoveryDiagnostics::new();
    diag.final_phase = RecoveryPhase::ReadingSlots;

    let disk_size = params.disk_size;
    let page_size = if params.page_size == 0 { 4096 } else { params.page_size };

    let mut selector          = EpochSlotSelector::new(disk_size);
    let mut records: [Option<EpochRecord>; 3] = [None, None, None];

    // ── Étape 1 : lecture et validation des 3 slots ─────────────────────────
    let slots = [EpochSlot::A, EpochSlot::B, EpochSlot::C];
    for &slot in &slots {
        let idx = slot as usize;
        let offset = match slot.disk_offset(disk_size) {
            Ok(o)  => o,
            Err(_) => {
                diag.slot_results[idx] = SlotCheckResult::IoError;
                selector.update_slot(slot, false, 0);
                continue;
            }
        };

        let mut buf = [0u8; 104];
        match (params.read_fn)(offset, &mut buf) {
            Ok(()) => {}
            Err(_) => {
                diag.slot_results[idx] = SlotCheckResult::IoError;
                selector.update_slot(slot, false, 0);
                EPOCH_STATS.inc_recovery_slot_io_errors();
                continue;
            }
        }

        match parse_slot_data(&buf) {
            Ok(Some(record)) => {
                let eid = record.epoch_id;
                selector.update_slot(slot, true, eid);
                diag.slot_results[idx] = SlotCheckResult::Valid(eid);
                records[idx] = Some(record);
            }
            Ok(None) => {
                // Slot vide (nouveau volume, ou slot jamais encore écrit).
                diag.slot_results[idx] = SlotCheckResult::Empty;
                selector.update_slot(slot, false, 0);
            }
            Err(ExofsError::ChecksumMismatch) => {
                // RÈGLE CHAIN-01 : checksum invalide = slot corrompu.
                diag.slot_results[idx] = SlotCheckResult::ChecksumMismatch;
                selector.update_slot(slot, false, 0);
                EPOCH_STATS.inc_recovery_checksum_errors();
            }
            Err(ExofsError::MagicMismatch) => {
                diag.slot_results[idx] = SlotCheckResult::MagicMismatch;
                selector.update_slot(slot, false, 0);
                EPOCH_STATS.inc_recovery_slot_magic_errors();
            }
            Err(e) => return Err(e),
        }
    }

    // ── Étape 2 : sélection du slot actif ──────────────────────────────────
    diag.final_phase = RecoveryPhase::SelectingActiveSlot;

    let valid_count = selector.valid_count();
    if valid_count == 0 {
        diag.final_phase = RecoveryPhase::Failed;
        return Err(ExofsError::NoValidEpoch);
    }

    let (active_slot, active_epoch_raw) = selector
        .find_latest_valid_slot()
        .ok_or(ExofsError::NoValidEpoch)?;

    let active_record = records[active_slot as usize]
        .as_ref()
        .ok_or(ExofsError::CorruptedStructure)?;

    // ── Étape 3 : vérification de l'EpochRoot pointé (CHAIN-01) ────────────
    diag.final_phase = RecoveryPhase::VerifyingEpochRoot;
    let root_offset = DiskOffset(active_record.root_offset);

    if root_offset.0 != 0 {
        // Alloue une page tampon — préréservée (OOM-02).
        let mut page_buf: Vec<u8> = Vec::new();
        page_buf
            .try_reserve(page_size)
            .map_err(|_| ExofsError::NoMemory)?;
        page_buf.resize(page_size, 0u8);

        (params.read_page)(root_offset, &mut page_buf, page_size)?;

        // RÈGLE CHAIN-01 : magic AVANT checksum.
        verify_epoch_root_page(&page_buf)?;
        diag.epoch_root_valid = true;
    } else {
        // Offset zéro = epoch vide (premier commit jamais créé).
        diag.epoch_root_valid = true;
    }

    // ── Étape 4 : replay du redo log si crash précédent ────────────────────
    let needs_redo = active_record.is_recovering();
    if needs_redo {
        diag.final_phase = RecoveryPhase::ReplayingRedoLog;
        let replayed = redo_log_replay(active_record, params.read_page, page_size)?;
        diag.redo_epochs_replayed = replayed;
        EPOCH_STATS.inc_recovery_epochs_replayed();
    }

    // ── Étape 5 : mise à jour du superblock in-memory ───────────────────────
    diag.final_phase = RecoveryPhase::UpdatingSuperblock;

    let active_epoch_id = EpochId(active_epoch_raw);
    (params.set_epoch_fn)(active_epoch_id)?;

    // ── Statistiques ────────────────────────────────────────────────────────
    if valid_count < 3 {
        diag.degraded_mode = true;
        EPOCH_STATS.inc_recovery_degraded_mounts();
    }

    let end_tsc = (params.tsc_fn)();
    diag.duration_cycles = end_tsc.saturating_sub(start_tsc);
    diag.final_phase = RecoveryPhase::Complete;

    Ok(RecoveryResult {
        active_epoch_id,
        active_slot,
        valid_slot_count: valid_count,
        slot_selector: selector,
        needs_redo,
        diagnostics: diag,
    })
}

// =============================================================================
// Replay du redo log
// =============================================================================

/// Rejoue les epochs non commités depuis le redo log de l'EpochRecord actif.
///
/// Retourne le nombre de pages root effectivement parcourues.
///
/// # Notes de conception
/// - Si l'EpochRecord a le flag RECOVERING, cela signifie qu'un commit était
///   en cours au moment du crash. On vérifie et compte les pages valides.
/// - Cette fonction est conservatrice : elle ne modifie aucun état disque.
/// - RECUR-01 : pas de récursion, boucle iterative uniquement.
fn redo_log_replay(
    record:    &EpochRecord,
    read_page: ReadPageFn<'_>,
    page_size: usize,
) -> ExofsResult<u32> {
    let root_offset = DiskOffset(record.root_offset);
    if root_offset.0 == 0 {
        return Ok(0);
    }

    let mut current_offset = root_offset;
    let mut pages_replayed: u32 = 0;
    // Limite de sécurité anti-boucle infinie sur chaîne corrompue.
    let max_pages: u32 = 65536;

    loop {
        if pages_replayed >= max_pages {
            break;
        }

        let mut page_buf: Vec<u8> = Vec::new();
        page_buf
            .try_reserve(page_size)
            .map_err(|_| ExofsError::NoMemory)?;
        page_buf.resize(page_size, 0u8);

        match read_page(current_offset, &mut page_buf, page_size) {
            Ok(()) => {}
            Err(_) => break,
        }

        // CHAIN-01 : magic avant checksum.
        match verify_epoch_root_page(&page_buf) {
            Ok(()) => {}
            Err(_) => break,
        }

        pages_replayed = pages_replayed.saturating_add(1);

        // Lecture du pointeur next_page depuis l'en-tête.
        // Format EpochRootPageHeader : magic(4)+checksum(32)+epoch_id(8)+next_page(8)+...
        // next_page field offset = 4 + 32 + 8 = 44.
        if page_buf.len() < 52 {
            break;
        }
        let next_offset_bytes: [u8; 8] = {
            let mut arr = [0u8; 8];
            arr.copy_from_slice(&page_buf[44..52]);
            arr
        };
        let next_offset = u64::from_le_bytes(next_offset_bytes);

        const CHAIN_PLACEHOLDER: u64 = 0xDEAD_BEEF_DEAD_BEEF;
        if next_offset == 0 || next_offset == CHAIN_PLACEHOLDER {
            break;
        }

        current_offset = DiskOffset(next_offset);
    }

    Ok(pages_replayed)
}

// =============================================================================
// Helpers de diagnostic
// =============================================================================

/// Retourne `true` si moins de 3 slots sont valides.
#[inline]
pub fn is_degraded_recovery(results: &[SlotCheckResult; 3]) -> bool {
    let valid = results
        .iter()
        .filter(|r| matches!(r, SlotCheckResult::Valid(_)))
        .count();
    valid < 3
}

/// Extrait l'EpochId maximal parmi les slots valides.
///
/// Retourne `None` si aucun slot n'est valide.
pub fn max_valid_epoch(results: &[SlotCheckResult; 3]) -> Option<u64> {
    results
        .iter()
        .filter_map(|r| {
            if let SlotCheckResult::Valid(e) = r {
                Some(*e)
            } else {
                None
            }
        })
        .max()
}

// =============================================================================
// Statistiques de récupération
// =============================================================================

/// Snapshot immutable des statistiques de récupération.
#[derive(Debug, Copy, Clone)]
pub struct RecoveryStats {
    /// Nombre de montages dégradés (< 3 slots valides).
    pub degraded_mounts:   u64,
    /// Nombre d'epochs rejoués suite à un crash.
    pub epochs_replayed:   u64,
    /// Erreurs I/O sur les slots.
    pub slot_io_errors:    u64,
    /// Erreurs de checksum sur les slots.
    pub checksum_errors:   u64,
    /// Erreurs de magic sur les slots.
    pub slot_magic_errors: u64,
}

impl fmt::Display for RecoveryStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "RecoveryStats {{ degraded={}, replayed={}, io_err={}, \
             checksum_err={}, magic_err={} }}",
            self.degraded_mounts,
            self.epochs_replayed,
            self.slot_io_errors,
            self.checksum_errors,
            self.slot_magic_errors,
        )
    }
}

/// Collecte un snapshot des statistiques de récupération depuis EPOCH_STATS.
pub fn snapshot_recovery_stats() -> RecoveryStats {
    let snap = EPOCH_STATS.snapshot();
    RecoveryStats {
        degraded_mounts:   snap.recovery.degraded_mounts,
        epochs_replayed:   snap.recovery.epochs_replayed,
        slot_io_errors:    snap.recovery.slot_io_errors,
        checksum_errors:   snap.recovery.checksum_errors,
        slot_magic_errors: snap.recovery.slot_magic_errors,
    }
}

// =============================================================================
// Validation post-récupération
// =============================================================================

/// Valide la cohérence d'un RecoveryResult.
///
/// Vérifie :
/// - valid_slot_count ∈ [1, 3].
/// - active_epoch_id != EpochId(0).
/// - diagnostics.final_phase == Complete.
pub fn validate_recovery_result(result: &RecoveryResult) -> ExofsResult<()> {
    if result.valid_slot_count == 0 || result.valid_slot_count > 3 {
        return Err(ExofsError::CorruptedStructure);
    }
    if result.active_epoch_id.0 == 0 {
        return Err(ExofsError::CorruptedStructure);
    }
    if result.diagnostics.final_phase != RecoveryPhase::Complete {
        return Err(ExofsError::CorruptedStructure);
    }
    Ok(())
}

/// Résumé compact d'une récupération pour affichage kernel log.
#[derive(Debug, Copy, Clone)]
pub struct RecoverySummary {
    pub active_epoch_id:  EpochId,
    pub active_slot:      EpochSlot,
    pub valid_slot_count: u8,
    pub degraded:         bool,
    pub redo_replayed:    u32,
}

impl fmt::Display for RecoverySummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Recovery OK: epoch={} slot={:?} valid={}/3{} redo={}",
            self.active_epoch_id.0,
            self.active_slot,
            self.valid_slot_count,
            if self.degraded { " [DEGRADED]" } else { "" },
            self.redo_replayed,
        )
    }
}

/// Construit un RecoverySummary depuis un RecoveryResult.
pub fn recovery_summary(result: &RecoveryResult) -> RecoverySummary {
    RecoverySummary {
        active_epoch_id:  result.active_epoch_id,
        active_slot:      result.active_slot,
        valid_slot_count: result.valid_slot_count,
        degraded:         result.diagnostics.degraded_mode,
        redo_replayed:    result.diagnostics.redo_epochs_replayed,
    }
}
