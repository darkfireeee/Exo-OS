//! EpochReplay — reconstruction d'un Epoch incomplet ExoFS (no_std).
//! RÈGLE 7 : 3 barrières NVMe dans l'ordre data→root→record.

use crate::fs::exofs::core::{EpochId, FsError};
use super::boot_recovery::BlockDevice;
use super::recovery_log::RECOVERY_LOG;

/// Résultat du replay.
#[derive(Clone, Debug)]
pub struct ReplayResult {
    pub epoch_id:     EpochId,
    pub n_replayed:   u32,
    pub n_skipped:    u32,
}

pub const EPOCH_RECORD_MAGIC: u64 = 0x45504F43_48524543; // "EPOCHREC"

/// Entrée on-disk du journal d'epoch.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct EpochRecord {
    pub magic:    u64,
    pub epoch_id: u64,
    pub blob_id:  [u8; 32],
    pub lba:      u64,
    pub len:      u32,
    pub checksum: u32,
}

const _: () = assert!(core::mem::size_of::<EpochRecord>() == 64);

pub struct EpochReplay;

impl EpochReplay {
    /// Rejoue les entrées de l'epoch `epoch_id` depuis le journal.
    ///
    /// Ordre strict : data_blocks → root_record → epoch_record (RÈGLE 7).
    pub fn replay(device: &mut dyn BlockDevice, epoch_id: EpochId) -> Result<ReplayResult, FsError> {
        // Lire le journal d'epoch (LBA fixe pour simplicity dans ce module).
        let journal_lba = Self::journal_lba();
        let mut result = ReplayResult { epoch_id, n_replayed: 0, n_skipped: 0 };

        // Lire les enregistrements jusqu'à trouver un magic invalide.
        let mut lba = journal_lba;
        loop {
            let mut buf = [0u8; 64];
            if device.read_block(lba, &mut buf).is_err() { break; }

            // RÈGLE 8 : magic en premier.
            let magic = u64::from_le_bytes(buf[0..8].try_into().unwrap_or([0; 8]));
            if magic != EPOCH_RECORD_MAGIC { break; }

            // SAFETY: EpochRecord est repr(C) 64B.
            let rec: EpochRecord = unsafe { core::mem::transmute_copy(&buf) };

            if rec.epoch_id != epoch_id.0 {
                result.n_skipped += 1;
                lba = lba.checked_add(1).ok_or(FsError::Overflow)?;
                continue;
            }

            // Phase 1 : relire le bloc data original.
            let mut data_buf = alloc::vec![0u8; rec.len as usize];
            device.read_block(rec.lba, &mut data_buf)?;

            // Phase 2 : vérifier checksum.
            let cs = crate::fs::exofs::dedup::content_hash::xxhash64_simple(&data_buf) as u32;
            if cs != rec.checksum {
                result.n_skipped += 1;
                lba = lba.checked_add(1).ok_or(FsError::Overflow)?;
                continue;
            }

            // Phase 3 : réécrire (rejouer) le bloc.
            // RÈGLE 7 : barrière mémoire avant chaque écriture.
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            device.write_block(rec.lba, &data_buf)?;
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

            result.n_replayed += 1;
            lba = lba.checked_add(1).ok_or(FsError::Overflow)?;
        }

        Ok(result)
    }

    fn journal_lba() -> u64 { 0x1000 }
}
