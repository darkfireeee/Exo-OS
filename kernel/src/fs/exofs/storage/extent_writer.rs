//! extent_writer.rs — Écritures d'extents vers le heap ExoFS (no_std).
//! RÈGLE 14 : tous les calculs d'offset utilisent checked_add/checked_mul.

use alloc::vec::Vec;
use crate::fs::exofs::core::FsError;
use crate::fs::exofs::epoch::epoch_id::EpochId;

pub const EXTENT_MAGIC: u32 = 0x45585453; // "EXTS"

/// En-tête on-disk d'un extent.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ExtentHeader {
    pub magic:       u32,
    pub epoch_id:    u64,
    pub offset:      u64,   // Offset logique dans l'objet (octets).
    pub length:      u32,   // Taille des données (octets).
    pub checksum:    u32,
}

const _: () = assert!(core::mem::size_of::<ExtentHeader>() == 28);

/// Résultat d'écriture d'un extent.
#[derive(Clone, Debug)]
pub struct ExtentWriteResult {
    pub heap_offset:  u64,   // Offset physique dans le heap.
    pub total_bytes:  u64,   // Header + data.
}

/// Sérialise et écrit un extent dans `out`.
/// `logical_offset` : position dans l'objet, `data` : contenu.
pub fn write_extent(
    logical_offset: u64,
    data:           &[u8],
    epoch_id:       u64,
    heap_offset:    u64,
    out:            &mut Vec<u8>,
) -> Result<ExtentWriteResult, FsError> {
    // RÈGLE 14 : vérification overflow.
    let total = (core::mem::size_of::<ExtentHeader>() as u64)
        .checked_add(data.len() as u64)
        .ok_or(FsError::Overflow)?;

    let checksum = crc32_data(data);
    let header   = ExtentHeader {
        magic:    EXTENT_MAGIC,
        epoch_id,
        offset:   logical_offset,
        length:   data.len() as u32,
        checksum,
    };

    out.try_reserve(total as usize).map_err(|_| FsError::OutOfMemory)?;

    // SAFETY: ExtentHeader est repr(C) 28B.
    let hdr_bytes: &[u8] = unsafe {
        core::slice::from_raw_parts(
            &header as *const _ as *const u8,
            core::mem::size_of::<ExtentHeader>(),
        )
    };
    out.extend_from_slice(hdr_bytes);
    out.extend_from_slice(data);

    Ok(ExtentWriteResult { heap_offset, total_bytes: total })
}

fn crc32_data(data: &[u8]) -> u32 {
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
