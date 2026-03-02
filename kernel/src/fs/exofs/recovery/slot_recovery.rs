//! SlotRecovery — sélection du slot A/B/C le plus récent valide ExoFS (no_std).
//! RÈGLE 8 : vérification magic EN PREMIER sur chaque slot.

use crate::fs::exofs::core::{EpochId, FsError};
use super::boot_recovery::BlockDevice;

/// Identifiant de slot (0=A, 1=B, 2=C).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotId(pub u8);

pub const SLOT_A: SlotId = SlotId(0);
pub const SLOT_B: SlotId = SlotId(1);
pub const SLOT_C: SlotId = SlotId(2);

pub const SLOT_MAGIC: u64    = 0x45584F46_5F534C54; // "EXOF_SLT"
pub const SLOT_HEADER_SIZE:  usize = 64;

/// En-tête on-disk d'un slot.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SlotHeader {
    pub magic:       u64,
    pub slot_id:     u8,
    pub _pad:        [u8; 7],
    pub epoch_id:    u64,
    pub root_blob:   [u8; 32],
    pub dirty_flag:  u8,
    pub _pad2:       [u8; 7],
}

const _: () = assert!(core::mem::size_of::<SlotHeader>() == 64);

/// Résultat de sélection de slot.
#[derive(Clone, Debug)]
pub struct SlotRecoveryResult {
    pub selected_slot: SlotId,
    pub epoch_id:      EpochId,
    pub dirty_flag:    bool,
    pub needs_replay:  bool,
}

/// LBA de début de chaque slot (configurable, valeurs par défaut).
const SLOT_LBAS: [u64; 3] = [0x100, 0x200, 0x300];

pub struct SlotRecovery;

impl SlotRecovery {
    /// Lit et valide les 3 slots, retourne celui avec le max epoch_id valide.
    pub fn select_best(device: &dyn BlockDevice) -> Result<SlotRecoveryResult, FsError> {
        let mut candidates: [(bool, SlotHeader); 3] = [(false, unsafe { core::mem::zeroed() }); 3];

        for i in 0..3 {
            let mut buf = [0u8; 64];
            if device.read_block(SLOT_LBAS[i], &mut buf).is_ok() {
                // RÈGLE 8 : magic EN PREMIER.
                let magic = u64::from_le_bytes(buf[0..8].try_into().unwrap_or([0; 8]));
                if magic == SLOT_MAGIC {
                    // SAFETY: SlotHeader est repr(C) 64B, buffer aligné.
                    let header: SlotHeader = unsafe { core::mem::transmute_copy(&buf) };
                    candidates[i] = (true, header);
                }
            }
        }

        // Sélectionner le max epoch valide.
        let mut best: Option<(usize, &SlotHeader)> = None;
        for (i, (valid, header)) in candidates.iter().enumerate() {
            if !valid { continue; }
            best = match best {
                None => Some((i, header)),
                Some((_, bh)) if header.epoch_id > bh.epoch_id => Some((i, header)),
                other => other,
            };
        }

        let (idx, header) = best.ok_or(FsError::InvalidMagic)?;
        let dirty_flag   = header.dirty_flag != 0;
        let needs_replay = dirty_flag;

        Ok(SlotRecoveryResult {
            selected_slot: SlotId(idx as u8),
            epoch_id:      EpochId(header.epoch_id),
            dirty_flag,
            needs_replay,
        })
    }

    /// Écrit un slot (mise à jour après commit epoch).
    pub fn write_slot(
        device:   &mut dyn BlockDevice,
        slot_id:  SlotId,
        header:   &SlotHeader,
    ) -> Result<(), FsError> {
        let buf: [u8; 64] = unsafe { core::mem::transmute_copy(header) };
        device.write_block(SLOT_LBAS[slot_id.0 as usize], &buf)
    }
}
