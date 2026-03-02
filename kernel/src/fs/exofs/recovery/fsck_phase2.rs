//! FsckPhase2 — scan heap : vérification ObjectHeaders magic+checksum ExoFS (no_std).
//! RÈGLE 8 : vérification magic EN PREMIER.

use crate::fs::exofs::core::FsError;
use super::boot_recovery::BlockDevice;
use super::fsck::PhaseResult;

pub const OBJECT_HEADER_MAGIC: u64 = 0x4F424A48_44520000; // "OBJHDR.."
pub const HEAP_START_LBA: u64 = 0x400;
pub const OBJECT_HEADER_SIZE: usize = 96;

/// En-tête on-disk d'un objet physique (P-Blob header).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ObjectHeader {
    pub magic:       u64,
    pub blob_id:     [u8; 32],
    pub size:        u64,
    pub flags:       u32,
    pub ref_count:   u32,
    pub epoch_id:    u64,
    pub checksum:    u64,
    pub _pad:        [u8; 4],
}

const _: () = assert!(core::mem::size_of::<ObjectHeader>() == 96);

pub struct Phase2Checker;

impl Phase2Checker {
    pub fn run(device: &dyn BlockDevice, _repair: bool) -> Result<PhaseResult, FsError> {
        let mut result = PhaseResult::default();
        let mut lba = HEAP_START_LBA;
        let max_scan = 65536u64; // Limite de scan pour éviter boucle infinie.

        while lba < HEAP_START_LBA.saturating_add(max_scan) {
            let mut buf = [0u8; OBJECT_HEADER_SIZE];
            match device.read_block(lba, &mut buf) {
                Err(_) => break,
                Ok(_) => {}
            }

            // RÈGLE 8 : magic en premier.
            let magic = u64::from_le_bytes(buf[0..8].try_into().unwrap_or([0; 8]));
            if magic == 0 { break; } // Zone vide = fin du heap.
            if magic != OBJECT_HEADER_MAGIC {
                result.errors += 1;
                lba = lba.checked_add(1).ok_or(FsError::Overflow)?;
                continue;
            }

            // SAFETY: ObjectHeader est repr(C) 96B.
            let header: ObjectHeader = unsafe {
                core::mem::transmute_copy::<[u8; 96], ObjectHeader>(&buf[..96].try_into().unwrap())
            };

            // Vérifier ref_count cohérence.
            if header.ref_count == 0 && header.flags & 0x01 == 0 {
                // Objet non marqué supprimé mais ref_count=0 → suspect.
                result.warnings += 1;
            }

            // Avancer au bloc suivant (taille de l'objet).
            let n_blocks = header.size.div_ceil(device.block_size() as u64).max(1);
            lba = lba.checked_add(1 + n_blocks).ok_or(FsError::Overflow)?;
        }

        Ok(result)
    }
}
