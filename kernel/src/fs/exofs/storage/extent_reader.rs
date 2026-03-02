//! extent_reader.rs — Lecture et validation d'extents depuis le heap ExoFS (no_std).
//! RÈGLE 8 : vérification magic EN PREMIER.

use alloc::vec::Vec;
use crate::fs::exofs::core::FsError;
use super::extent_writer::{ExtentHeader, EXTENT_MAGIC};

/// Résultat de lecture d'un extent.
#[derive(Clone, Debug)]
pub struct ExtentReadResult {
    pub logical_offset: u64,
    pub epoch_id:       u64,
    pub data:           Vec<u8>,
}

/// Désérialise un extent depuis le buffer `buf` (doit commencer à l'en-tête).
/// RÈGLE 8 : magic vérifié en premier.
pub fn read_extent(buf: &[u8]) -> Result<ExtentReadResult, FsError> {
    let hdr_size = core::mem::size_of::<ExtentHeader>();
    if buf.len() < hdr_size {
        return Err(FsError::InvalidData);
    }

    // RÈGLE 8 : magic EN PREMIER.
    let magic = u32::from_le_bytes(buf[0..4].try_into().unwrap_or([0; 4]));
    if magic != EXTENT_MAGIC {
        return Err(FsError::InvalidMagic);
    }

    // SAFETY: ExtentHeader est repr(C) 28B, buf est assez grand.
    let header: ExtentHeader = unsafe { core::mem::transmute_copy(&*(buf.as_ptr() as *const [u8; 28])) };

    let data_len = header.length as usize;
    let end = hdr_size.checked_add(data_len).ok_or(FsError::Overflow)?;
    if buf.len() < end {
        return Err(FsError::InvalidData);
    }

    // Vérifier le checksum.
    let data_slice = &buf[hdr_size..end];
    let computed   = crc32_data(data_slice);
    if computed != header.checksum {
        return Err(FsError::IntegrityCheckFailed);
    }

    let mut data = Vec::new();
    data.try_reserve(data_len).map_err(|_| FsError::OutOfMemory)?;
    data.extend_from_slice(data_slice);

    Ok(ExtentReadResult {
        logical_offset: header.offset,
        epoch_id:       header.epoch_id,
        data,
    })
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
