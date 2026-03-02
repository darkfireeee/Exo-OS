//! FsckPhase4 — détection des orphelins (blobs non atteints depuis les racines).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use crate::fs::exofs::core::FsError;
use super::boot_recovery::BlockDevice;
use super::fsck::PhaseResult;
use super::fsck_phase2::{HEAP_START_LBA, OBJECT_HEADER_MAGIC, OBJECT_HEADER_SIZE, ObjectHeader};

/// Blob orphelin identifié.
#[derive(Clone, Copy, Debug)]
pub struct OrphanBlob {
    pub blob_id: [u8; 32],
    pub lba:     u64,
    pub size:    u64,
}

pub struct Phase4Checker;

impl Phase4Checker {
    pub fn run(device: &dyn BlockDevice, repair: bool) -> Result<PhaseResult, FsError> {
        let mut result = PhaseResult::default();
        let mut all_blobs: Vec<OrphanBlob> = Vec::new();
        let mut lba = HEAP_START_LBA;
        let max_scan = 65536u64;

        // Scanner tous les blobs.
        while lba < HEAP_START_LBA.saturating_add(max_scan) {
            let mut buf = [0u8; OBJECT_HEADER_SIZE];
            if device.read_block(lba, &mut buf).is_err() { break; }

            let magic = u64::from_le_bytes(buf[0..8].try_into().unwrap_or([0; 8]));
            if magic == 0 { break; }
            if magic != OBJECT_HEADER_MAGIC {
                lba = lba.checked_add(1).ok_or(FsError::Overflow)?;
                continue;
            }

            // SAFETY: ObjectHeader repr(C) 96B.
            let header: ObjectHeader = unsafe {
                core::mem::transmute_copy::<[u8; 96], ObjectHeader>(&buf[..96].try_into().unwrap())
            };

            // Un blob avec ref_count=0 et non marqué supprimé est orphelin.
            if header.ref_count == 0 && header.flags & 0x02 == 0 {
                if all_blobs.try_reserve(1).is_ok() {
                    all_blobs.push(OrphanBlob {
                        blob_id: header.blob_id,
                        lba,
                        size: header.size,
                    });
                }
                result.errors += 1;

                if repair {
                    // Marquer comme orphelin dans lost+found via fsck_repair.
                    if super::fsck_repair::FsckRepair::move_to_lost_found(device, lba).is_ok() {
                        result.repaired += 1;
                    }
                }
            }

            let n_blocks = header.size.div_ceil(device.block_size() as u64).max(1);
            lba = lba.checked_add(1 + n_blocks).ok_or(FsError::Overflow)?;
        }

        Ok(result)
    }
}
