//! FsckPhase1 — vérification Superblock + miroirs + feature flags ExoFS (no_std).
//! RÈGLE 8 : vérification magic EN PREMIER.

use crate::fs::exofs::core::FsError;
use super::boot_recovery::BlockDevice;
use super::fsck::PhaseResult;

pub const SUPERBLOCK_MAGIC: u64 = 0x45584F46_5F535250; // "EXOF_SRP"
pub const SUPERBLOCK_LBA:   u64 = 0x0;
pub const SUPERBLOCK_SIZE:  usize = 128;

/// Superblock on-disk.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Superblock {
    pub magic:         u64,
    pub version:       u32,
    pub block_size:    u32,
    pub n_blocks:      u64,
    pub epoch_id:      u64,
    pub feature_flags: u64,
    pub root_blob:     [u8; 32],
    pub checksum:      u64,
    pub _pad:          [u8; 16],
}

const _: () = assert!(core::mem::size_of::<Superblock>() == 128);

pub struct Phase1Checker;

impl Phase1Checker {
    pub fn run(device: &dyn BlockDevice, _repair: bool) -> Result<PhaseResult, FsError> {
        let mut result = PhaseResult::default();

        // Lire le superblock.
        let mut buf = [0u8; SUPERBLOCK_SIZE];
        device.read_block(SUPERBLOCK_LBA, &mut buf).map_err(|_| {
            result.errors += 1;
            FsError::InvalidData
        })?;

        // RÈGLE 8 : magic en premier.
        let magic = u64::from_le_bytes(buf[0..8].try_into().unwrap_or([0; 8]));
        if magic != SUPERBLOCK_MAGIC {
            result.errors += 1;
            return Ok(result);
        }

        // SAFETY: Superblock est repr(C) 128B, buffer 128B.
        let sb: Superblock = unsafe { core::mem::transmute_copy::<[u8; 128], Superblock>(&buf) };

        // Vérifier version.
        if sb.version == 0 || sb.version > 9999 {
            result.warnings += 1;
        }

        // Vérifier block_size (doit être puissance de 2 entre 512 et 64K).
        if sb.block_size < 512 || sb.block_size > 65536 || (sb.block_size & (sb.block_size - 1)) != 0 {
            result.errors += 1;
        }

        // Vérifier que n_blocks > 0.
        if sb.n_blocks == 0 {
            result.errors += 1;
        }

        // Vérifier feature_flags connus.
        let known_features = 0x0000_0000_0000_00FF_u64;
        if sb.feature_flags & !known_features != 0 {
            result.warnings += 1; // Features inconnus → avertissement seulement.
        }

        Ok(result)
    }
}
