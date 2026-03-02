//! SnapshotStreamer — export en flux d'un snapshot ExoFS (no_std).

use alloc::vec::Vec;
use crate::fs::exofs::core::{BlobId, FsError};
use super::snapshot::{Snapshot, SnapshotId, SNAPSHOT_MAGIC};
use super::snapshot_list::SNAPSHOT_LIST;
use super::snapshot_restore::SnapshotBlobSource;

pub const STREAM_CHUNK_MAGIC: u32 = 0x53544343; // "STCC"
pub const STREAM_END_MAGIC:   u32 = 0x5354454E; // "STEN"

/// En-tête d'un chunk de flux.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct StreamChunkHeader {
    pub magic:     u32,
    pub seq:       u32,
    pub blob_id:   [u8; 32],
    pub data_len:  u32,
    pub checksum:  u32,
}

const _: () = assert!(core::mem::size_of::<StreamChunkHeader>() == 48);

/// Callback de sortie du flux.
pub trait StreamWriter: Send + Sync {
    fn write_bytes(&mut self, data: &[u8]) -> Result<(), FsError>;
}

pub struct SnapshotStreamer;

impl SnapshotStreamer {
    /// Sérialise le snapshot `snap_id` vers `writer` en séquence de chunks.
    pub fn stream(
        snap_id: SnapshotId,
        source:  &dyn SnapshotBlobSource,
        writer:  &mut dyn StreamWriter,
    ) -> Result<u64, FsError> {
        let snap = SNAPSHOT_LIST.get(snap_id).ok_or(FsError::NotFound)?;
        let blobs = source.list_blobs(snap_id)?;

        // Écrire l'en-tête de snapshot.
        let snap_header = snap.to_header(0);
        // SAFETY: SnapshotHeader est repr(C) de taille fixe 168B.
        let header_bytes: &[u8] = unsafe {
            core::slice::from_raw_parts(
                &snap_header as *const _ as *const u8,
                core::mem::size_of_val(&snap_header),
            )
        };
        writer.write_bytes(header_bytes)?;

        let mut n_bytes = 0u64;
        for (seq, &blob_id) in blobs.iter().enumerate() {
            let data = source.read_blob(snap_id, blob_id)?;
            let checksum = Self::crc32_fast(&data);
            let header = StreamChunkHeader {
                magic:    STREAM_CHUNK_MAGIC,
                seq:      seq as u32,
                blob_id:  blob_id.as_bytes(),
                data_len: data.len() as u32,
                checksum,
            };
            // SAFETY: StreamChunkHeader est repr(C) de taille fixe 48B.
            let header_bytes: &[u8] = unsafe {
                core::slice::from_raw_parts(
                    &header as *const _ as *const u8,
                    core::mem::size_of_val(&header),
                )
            };
            writer.write_bytes(header_bytes)?;
            writer.write_bytes(&data)?;
            n_bytes = n_bytes.checked_add(data.len() as u64).ok_or(FsError::Overflow)?;
        }

        // Marqueur de fin.
        let end_magic = STREAM_END_MAGIC.to_le_bytes();
        writer.write_bytes(&end_magic)?;
        Ok(n_bytes)
    }

    /// CRC32 simplifié (Castagnoli polynomial).
    fn crc32_fast(data: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFF_FFFF;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                let mask = (0u32.wrapping_sub(crc & 1)) as u32;
                crc = (crc >> 1) ^ (0x82F6_3B78 & mask);
            }
        }
        !crc
    }
}
