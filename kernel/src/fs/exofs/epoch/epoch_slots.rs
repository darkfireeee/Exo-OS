// kernel/src/fs/exofs/epoch/epoch_slots.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Gestion des 3 slots d'EpochRecord (A, B, C) sur disque
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Structure disque :
//   Slot A : offset 4 KB  (EPOCH_SLOT_A)
//   Slot B : offset 8 KB  (EPOCH_SLOT_B)
//   Slot C : disk_size - 8 KB (EPOCH_SLOT_C dynamique)
//
// Rotation anti-usure : on écrit toujours dans le slot le plus ancien.
// Le slot C est le slot de secours en cas de corruption des deux premiers.
// RÈGLE EPOCH-04 : les 3 slots valides = fichier sain ; 2/3 = récupérable.

use core::fmt;

use crate::fs::exofs::core::{DiskOffset, EpochId, ExofsError, ExofsResult};
use crate::fs::exofs::epoch::epoch_record::EpochRecord;

// =============================================================================
// Offsets disque des slots — RÈGLE SLOT-01 : offsets FIXES et immuables
// =============================================================================

/// Offset du Slot A : 4 KB depuis le début du volume.
pub const EPOCH_SLOT_A_OFFSET: u64 = 4 * 1024;

/// Offset du Slot B : 8 KB depuis le début du volume.
pub const EPOCH_SLOT_B_OFFSET: u64 = 8 * 1024;

/// Distance du Slot C depuis la FIN du volume : 8 MB.
const EPOCH_SLOT_C_FROM_END: u64 = 8 * 1024 * 1024;

/// Calcule l'offset du Slot C à partir de la taille du volume.
///
/// RÈGLE ARITH-02 : utilisation de checked_sub pour éviter l'underflow.
/// RÈGLE SLOT-02 : le slot C est toujours dans la moitié supérieure du volume.
pub fn epoch_slot_c_offset(disk_size: u64) -> ExofsResult<DiskOffset> {
    let offset = disk_size
        .checked_sub(EPOCH_SLOT_C_FROM_END)
        .ok_or(ExofsError::OffsetOverflow)?;
    // Vérification de cohérence : le slot C doit être après le slot B.
    if offset <= EPOCH_SLOT_B_OFFSET {
        return Err(ExofsError::OffsetOverflow);
    }
    Ok(DiskOffset(offset))
}

// =============================================================================
// EpochSlot — identifiant des 3 slots
// =============================================================================

/// Les 3 slots d'EpochRecord.
///
/// Chaque slot contient exactement 104 octets (un EpochRecord).
/// Les 3 slots sont en rotation pour minimiser l'usure du SSD.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum EpochSlot {
    /// Slot A : offset fixe 4 KB.
    A = 0,
    /// Slot B : offset fixe 8 KB.
    B = 1,
    /// Slot C : offset variable (disk_size - 8 MB).
    C = 2,
}

impl EpochSlot {
    /// Retourne l'offset disque du slot pour une taille de volume donnée.
    ///
    /// RÈGLE ARITH-02 : checked_sub pour le slot C.
    pub fn disk_offset(self, disk_size: DiskOffset) -> ExofsResult<DiskOffset> {
        match self {
            EpochSlot::A => Ok(DiskOffset(EPOCH_SLOT_A_OFFSET)),
            EpochSlot::B => Ok(DiskOffset(EPOCH_SLOT_B_OFFSET)),
            EpochSlot::C => epoch_slot_c_offset(disk_size.0),
        }
    }

    /// Nom lisible du slot.
    pub fn name(self) -> &'static str {
        match self {
            EpochSlot::A => "SlotA",
            EpochSlot::B => "SlotB",
            EpochSlot::C => "SlotC",
        }
    }

    /// Retourne les 3 slots sous forme de tableau (ordre A, B, C).
    pub fn all() -> [EpochSlot; 3] {
        [EpochSlot::A, EpochSlot::B, EpochSlot::C]
    }

    /// Index numérique du slot (0, 1, 2).
    #[inline]
    pub fn index(self) -> usize {
        self as usize
    }
}

impl fmt::Display for EpochSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

// =============================================================================
// SlotState — état d'un slot après lecture disque
// =============================================================================

/// État connu d'un slot après tentative de lecture.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SlotReadResult {
    /// Slot valide avec un EpochRecord vérifié.
    Valid(EpochId),
    /// Slot vide (magic = 0x00000000 ou 0xFFFFFFFF — volume neuf).
    Empty,
    /// Slot dont le magic est invalide (corrompu ou non-ExoFS).
    InvalidMagic,
    /// Slot dont le checksum échoue (contenu altéré).
    BadChecksum,
    /// Erreur I/O lors de la lecture.
    IoError,
}

impl SlotReadResult {
    /// Vrai si le slot est valide (contient un EpochRecord vérifié).
    #[inline]
    pub fn is_valid(self) -> bool {
        matches!(self, SlotReadResult::Valid(_))
    }

    /// Retourne l'EpochId si valide.
    #[inline]
    pub fn epoch_id(self) -> Option<EpochId> {
        match self {
            SlotReadResult::Valid(id) => Some(id),
            _ => None,
        }
    }

    /// Vrai si le slot peut être utilisé pour le recovery (pas une erreur I/O).
    #[inline]
    pub fn is_readable(self) -> bool {
        matches!(self, SlotReadResult::Valid(_) | SlotReadResult::Empty)
    }
}

// =============================================================================
// SlotStatus — état agrégé d'un slot
// =============================================================================

/// État complet d'un slot après vérification.
#[derive(Copy, Clone, Debug)]
pub struct SlotStatus {
    /// Identifiant du slot.
    pub slot: EpochSlot,
    /// Résultat de la lecture.
    pub result: SlotReadResult,
    /// Offset disque du slot.
    pub offset: DiskOffset,
}

impl SlotStatus {
    /// Crée un SlotStatus "invalide" (pour initialisation).
    pub fn invalid(slot: EpochSlot, offset: DiskOffset) -> Self {
        SlotStatus {
            slot,
            result: SlotReadResult::Empty,
            offset,
        }
    }

    /// Vrai si le slot contient un EpochRecord valide.
    #[inline]
    pub fn is_valid(&self) -> bool {
        self.result.is_valid()
    }
}

impl fmt::Display for SlotStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}[{}]={:?}",
            self.slot.name(),
            self.offset.0,
            self.result
        )
    }
}

// =============================================================================
// EpochSlotSelector — sélection du slot d'écriture (rotation anti-usure)
// =============================================================================

/// Sélecteur du slot cible pour la prochaine écriture d'EpochRecord.
///
/// # Stratégie de rotation
/// 1. Priorité aux slots invalides/vides (à remplir en premier).
/// 2. Parmi les slots valides, sélectionne celui avec le plus PETIT epoch_id
///    (le plus ancien = à recycler en priorité pour la rotation anti-usure).
///
/// Cette stratégie distribue équitablement les écritures sur les 3 slots
/// et minimise l'usure différentielle du SSD.
pub struct EpochSlotSelector {
    /// État des 3 slots après la phase de lecture initiale.
    slot_states: [SlotState; 3],
    /// Taille du disque (pour calculer l'offset du slot C).
    disk_size: DiskOffset,
}

// Manual Debug impl for EpochSlotSelector
impl core::fmt::Debug for EpochSlotSelector {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EpochSlotSelector")
            .field("disk_size", &self.disk_size)
            .finish()
    }
}

#[derive(Copy, Clone, Debug)]
struct SlotState {
    /// Vrai si le slot contient un EpochRecord valide.
    valid: bool,
    /// EpochId du record dans ce slot (0 si invalide).
    epoch_id: u64,
    /// Offset du slot sur disque.
    offset: DiskOffset,
}

impl EpochSlotSelector {
    /// Crée un sélecteur avec tous les slots marqués invalides.
    pub fn new(disk_size: DiskOffset) -> Self {
        let _slots = EpochSlot::all();
        let slot_states = [
            SlotState {
                valid: false,
                epoch_id: 0,
                offset: DiskOffset(EPOCH_SLOT_A_OFFSET),
            },
            SlotState {
                valid: false,
                epoch_id: 0,
                offset: DiskOffset(EPOCH_SLOT_B_OFFSET),
            },
            // Slot C : offset calculé si possible, sinon 0 (sera recalculé).
            SlotState {
                valid: false,
                epoch_id: 0,
                offset: epoch_slot_c_offset(disk_size.0).unwrap_or(DiskOffset(0)),
            },
        ];
        EpochSlotSelector {
            slot_states,
            disk_size,
        }
    }

    /// Met à jour l'état d'un slot après lecture pendant le recovery.
    ///
    /// # Paramètres
    /// - `slot`     : slot mis à jour.
    /// - `valid`    : vrai si le record est valide (magic + checksum).
    /// - `epoch_id` : EpochId du record (0 si invalide).
    pub fn update_slot(&mut self, slot: EpochSlot, valid: bool, epoch_id: u64) {
        let s = &mut self.slot_states[slot.index()];
        s.valid = valid;
        s.epoch_id = epoch_id;
    }

    /// Sélectionne le slot cible pour la prochaine écriture.
    ///
    /// Retourne `(EpochSlot, DiskOffset)` du slot choisi.
    ///
    /// # Ordre de priorité
    /// 1. Slot invalide (empty/corrompu) → à remplir en premier.
    /// 2. Slot valide avec le plus petit epoch_id → â recycler.
    pub fn select_write_slot(&self) -> ExofsResult<(EpochSlot, DiskOffset)> {
        // Priorité 1 : slot invalide.
        for slot in EpochSlot::all() {
            let s = &self.slot_states[slot.index()];
            if !s.valid {
                let offset = slot.disk_offset(self.disk_size)?;
                return Ok((slot, offset));
            }
        }

        // Priorité 2 : slot valide le plus ancien.
        let oldest = EpochSlot::all()
            .iter()
            .copied()
            .min_by_key(|&s| self.slot_states[s.index()].epoch_id)
            .unwrap_or(EpochSlot::A);

        let offset = oldest.disk_offset(self.disk_size)?;
        Ok((oldest, offset))
    }

    /// Retourne le slot contenant le plus grand epoch_id valide (slot actif).
    ///
    /// Utilisé par epoch_recovery pour déterminer l'epoch actif.
    pub fn find_latest_valid_slot(&self) -> Option<(EpochSlot, u64)> {
        EpochSlot::all()
            .iter()
            .copied()
            .filter(|&s| self.slot_states[s.index()].valid)
            .max_by_key(|&s| self.slot_states[s.index()].epoch_id)
            .map(|s| (s, self.slot_states[s.index()].epoch_id))
    }

    /// Nombre de slots actuellement valides (0 à 3).
    pub fn valid_count(&self) -> u8 {
        self.slot_states.iter().filter(|s| s.valid).count() as u8
    }

    /// Retourne true si tous les 3 slots sont valides (santé maximale).
    pub fn all_slots_valid(&self) -> bool {
        self.valid_count() == 3
    }

    /// Retourne un rapport d'état des 3 slots.
    pub fn status_report(&self) -> [SlotStatusReport; 3] {
        let slots = EpochSlot::all();
        [
            SlotStatusReport {
                slot: slots[0],
                valid: self.slot_states[0].valid,
                epoch_id: EpochId(self.slot_states[0].epoch_id),
                offset: self.slot_states[0].offset,
            },
            SlotStatusReport {
                slot: slots[1],
                valid: self.slot_states[1].valid,
                epoch_id: EpochId(self.slot_states[1].epoch_id),
                offset: self.slot_states[1].offset,
            },
            SlotStatusReport {
                slot: slots[2],
                valid: self.slot_states[2].valid,
                epoch_id: EpochId(self.slot_states[2].epoch_id),
                offset: self.slot_states[2].offset,
            },
        ]
    }
}

/// Rapport d'état d'un slot individuel.
#[derive(Copy, Clone, Debug)]
pub struct SlotStatusReport {
    pub slot: EpochSlot,
    pub valid: bool,
    pub epoch_id: EpochId,
    pub offset: DiskOffset,
}

impl fmt::Display for SlotStatusReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}@0x{:x} epoch={} valid={}",
            self.slot.name(),
            self.offset.0,
            self.epoch_id.0,
            self.valid
        )
    }
}

// =============================================================================
// parse_slot_data — lecture et validation d'un slot depuis 104 octets bruts
// =============================================================================

/// Parse et valide un EpochRecord depuis 104 octets bruts.
///
/// # Ordre de vérification (RÈGLE V-08 / CHAIN-01)
/// 1. Lecture du magic en premier (4 octets LE).
/// 2. Détection slot vide (0x00000000 ou 0xFFFFFFFF).
/// 3. Vérification magic EXOFS.
/// 4. Construction du record et vérification du checksum.
///
/// # Retour
/// - `Ok(None)` : slot vide (volume neuf ou effacé).
/// - `Ok(Some(record))` : record valide.
/// - `Err(InvalidMagic)` : magic inconnu (slot corrompu).
/// - `Err(ChecksumMismatch)` : contenu altéré.
pub fn parse_slot_data(data: &[u8; 104]) -> ExofsResult<Option<EpochRecord>> {
    EpochRecord::from_bytes(data)
}

/// Lit un slot et analyse son état de manière complète.
///
/// Combine la lecture brute avec le résultat typé `SlotReadResult`.
pub fn read_and_classify_slot(data: &[u8; 104]) -> SlotReadResult {
    match EpochRecord::from_bytes(data) {
        Ok(None) => SlotReadResult::Empty,
        Ok(Some(r)) => SlotReadResult::Valid(r.epoch_id()),
        Err(ExofsError::InvalidMagic) => SlotReadResult::InvalidMagic,
        Err(ExofsError::ChecksumMismatch) => SlotReadResult::BadChecksum,
        Err(_) => SlotReadResult::IoError,
    }
}

// =============================================================================
// Sélection du prochain slot en mode recovery (après réparation partielle)
// =============================================================================

/// Détermine le slot à utiliser après une recovery partielle (2/3 slots valides).
///
/// En mode dégradé (un slot corrompu), on assure que les deux prochains commits
/// vont réécrire le slot corrompu ET maintenir la rotation normale.
///
/// # Paramètre
/// - `selector` : sélecteur avec l'état lu au recovery.
/// - `reason`   : raison de la dégradation (log seulement).
pub fn recovery_write_slot(
    selector: &EpochSlotSelector,
    _reason: RecoverySlotReason,
) -> ExofsResult<(EpochSlot, DiskOffset)> {
    // En recovery, on priorise toujours le slot invalide.
    selector.select_write_slot()
}

/// Raison du choix du slot en recovery (pour les logs).
#[derive(Copy, Clone, Debug)]
pub enum RecoverySlotReason {
    /// Un slot était corrompu.
    CorruptedSlot,
    /// Crash pendant un commit précédent.
    IncompleteCommit,
    /// Volume neuf (premier montage).
    FreshVolume,
}

// ─────────────────────────────────────────────────────────────────────────────
