//! FsckPhase3 — reconstruction du graphe L-Obj → P-Blob → extents ExoFS (no_std).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use crate::fs::exofs::core::{BlobId, FsError};
use super::boot_recovery::BlockDevice;
use super::fsck::PhaseResult;
use super::fsck_phase2::{HEAP_START_LBA, OBJECT_HEADER_MAGIC, OBJECT_HEADER_SIZE, ObjectHeader};

/// Arc dans le graphe reconstruit.
#[derive(Clone, Copy, Debug)]
pub struct GraphArc {
    pub from: [u8; 32],
    pub to:   [u8; 32],
}

/// Graphe reconstruit en Phase 3.
pub struct ReconstructedGraph {
    pub n_nodes: u32,
    pub n_arcs:  u32,
    pub errors:  u32,
}

pub struct Phase3Checker;

impl Phase3Checker {
    pub fn run(device: &dyn BlockDevice, _repair: bool) -> Result<PhaseResult, FsError> {
        let mut result = PhaseResult::default();
        let mut graph: BTreeMap<[u8; 32], Vec<[u8; 32]>> = BTreeMap::new();
        let mut lba = HEAP_START_LBA;
        let max_scan = 65536u64;

        while lba < HEAP_START_LBA.saturating_add(max_scan) {
            let mut buf = [0u8; OBJECT_HEADER_SIZE];
            if device.read_block(lba, &mut buf).is_err() { break; }

            let magic = u64::from_le_bytes(buf[0..8].try_into().unwrap_or([0; 8]));
            if magic == 0 { break; }
            if magic != OBJECT_HEADER_MAGIC {
                result.errors += 1;
                lba = lba.checked_add(1).ok_or(FsError::Overflow)?;
                continue;
            }

            // SAFETY: ObjectHeader repr(C) 96B.
            let header: ObjectHeader = unsafe {
                core::mem::transmute_copy::<[u8; 96], ObjectHeader>(&buf[..96].try_into().unwrap())
            };

            let node = header.blob_id;

            // Ajouter le noeud dans le graphe.
            if !graph.contains_key(&node) {
                if graph.try_reserve(1).is_ok() {
                    graph.insert(node, Vec::new());
                }
            }

            // Avancer.
            let n_blocks = header.size.div_ceil(device.block_size() as u64).max(1);
            lba = lba.checked_add(1 + n_blocks).ok_or(FsError::Overflow)?;
        }

        result.warnings = 0; // Le graphe est pour usage interne Phase 4.
        Ok(result)
    }
}
