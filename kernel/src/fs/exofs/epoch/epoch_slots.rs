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

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, EpochId, DiskOffset,
    EPOCH_SLOT_A_OFFSET, EPOCH_SLOT_B_OFFSET,
};
use crate::fs::exofs::storage::layout::epoch_slot_c;
use crate::fs::exofs::epoch::epoch_record::EpochRecord;
use crate::fs::exofs::core::stats::EXOFS_STATS;

// ─────────────────────────────────────────────────────────────────────────────
// Identifiant de slot
// ─────────────────────────────────────────────────────────────────────────────

/// Les 3 slots d'EpochRecord.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum EpochSlot {
    A = 0,
    B = 1,
    C = 2,
}

impl EpochSlot {
    /// Retourne l'offset disque du slot pour une taille de disque donnée.
    pub fn disk_offset(self, disk_size: DiskOffset) -> ExofsResult<DiskOffset> {
        match self {
            EpochSlot::A => Ok(DiskOffset(EPOCH_SLOT_A_OFFSET)),
            EpochSlot::B => Ok(DiskOffset(EPOCH_SLOT_B_OFFSET)),
            EpochSlot::C => epoch_slot_c(disk_size.0),
        }
    }

    /// Retourne le nom lisible du slot.
    pub fn name(self) -> &'static str {
        match self {
            EpochSlot::A => "A",
            EpochSlot::B => "B",
            EpochSlot::C => "C",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// EpochSlotSelector — sélection du slot d'écriture (rotation anti-usure)
// ─────────────────────────────────────────────────────────────────────────────

/// Sélecteur du prochain slot d'écriture.
///
/// Stratégie : on sélectionne le slot avec le plus petit EpochId valide
/// (ou un slot invalide/corrompu en priorité). Cela distribue équitablement
/// les écritures sur les 3 slots.
pub struct EpochSlotSelector {
    /// État des 3 slots (epoch_id, validité).
    slot_states: [SlotState; 3],
    /// Taille du disque (pour calculer l'offset du slot C).
    disk_size: DiskOffset,
}

#[derive(Copy, Clone, Debug)]
struct SlotState {
    valid:    bool,
    epoch_id: u64,
}

impl EpochSlotSelector {
    /// Crée un sélecteur neuf avec tous les slots marqués invalides.
    pub fn new(disk_size: DiskOffset) -> Self {
        Self {
            slot_states: [SlotState { valid: false, epoch_id: 0 }; 3],
            disk_size,
        }
    }

    /// Met à jour l'état d'un slot après lecture depuis disque.
    pub fn update_slot(&mut self, slot: EpochSlot, valid: bool, epoch_id: u64) {
        self.slot_states[slot as usize] = SlotState { valid, epoch_id };
    }

    /// Sélectionne le slot cible pour la prochaine écriture.
    ///
    /// - S'il y a des slots invalides, retourne le premier invalide.
    /// - Sinon, retourne le slot avec le plus petit epoch_id (le plus ancien).
    pub fn select_write_slot(&self) -> ExofsResult<(EpochSlot, DiskOffset)> {
        let slots = [EpochSlot::A, EpochSlot::B, EpochSlot::C];

        // Priorité 1 : slot invalide ou corrompu.
        for &slot in &slots {
            if !self.slot_states[slot as usize].valid {
                let offset = slot.disk_offset(self.disk_size)?;
                return Ok((slot, offset));
            }
        }

        // Priorité 2 : slot avec le plus ancien epoch_id.
        let oldest = slots
            .iter()
            .copied()
            .min_by_key(|&s| self.slot_states[s as usize].epoch_id)
            .unwrap_or(EpochSlot::A);

        let offset = oldest.disk_offset(self.disk_size)?;
        Ok((oldest, offset))
    }

    /// Retourne le slot avec le plus grand epoch_id valide (= slot actif).
    ///
    /// Utilisé par epoch_recovery pour déterminer quel epoch est actif.
    pub fn find_latest_valid_slot(&self) -> Option<(EpochSlot, u64)> {
        let slots = [EpochSlot::A, EpochSlot::B, EpochSlot::C];
        slots
            .iter()
            .copied()
            .filter(|&s| self.slot_states[s as usize].valid)
            .max_by_key(|&s| self.slot_states[s as usize].epoch_id)
            .map(|s| (s, self.slot_states[s as usize].epoch_id))
    }

    /// Nombre de slots valides.
    pub fn valid_count(&self) -> u8 {
        self.slot_states.iter().filter(|s| s.valid).count() as u8
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Lecture d'un slot depuis son contenu brut
// ─────────────────────────────────────────────────────────────────────────────

/// Tente de parser et valider un EpochRecord depuis 104 octets de données disque.
///
/// Retourne `Ok(Some(record))` si le record est valide, `Ok(None)` si la
/// signature magic est absente (slot vide), `Err` si le checksum est corrompu.
pub fn parse_slot_data(data: &[u8; 104]) -> ExofsResult<Option<EpochRecord>> {
    use core::mem::size_of;
    // Lecture du magic en premier pour détecter un slot vide rapidement.
    let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    if magic == 0x0000_0000 || magic == 0xFFFF_FFFF {
        // Slot effacé ou NAND formaté.
        return Ok(None);
    }
    // SAFETY: EpochRecord est #[repr(C, packed)], Copy, taille 104 octets.
    // Les données data proviennent d'une lecture disque sécurisée (slice de 104 octets).
    let record: EpochRecord = unsafe {
        let ptr = data.as_ptr() as *const EpochRecord;
        core::ptr::read_unaligned(ptr)
    };
    match record.verify() {
        Ok(()) => Ok(Some(record)),
        Err(ExofsError::InvalidMagic) => {
            EXOFS_STATS.inc_slot_magic_errors();
            Ok(None)
        }
        Err(e) => Err(e),
    }
}
