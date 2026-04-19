//! block_io.rs — Helpers de lecture/écriture pour `BlockDevice`.
//!
//! Le contrat `BlockDevice` est bloc-orienté : `read_block` et `write_block`
//! attendent des buffers de taille exacte `block_size()`. Les phases de
//! recovery lisent cependant aussi des structures plus petites (64/96/128/256
//! octets) et des payloads de taille variable. Ce module encapsule le
//! read/modify/write nécessaire pour rester cohérent avec ce contrat.

extern crate alloc;

use alloc::vec;

use crate::fs::exofs::core::{ExofsError, ExofsResult};

use super::boot_recovery::BlockDevice;

fn block_size(device: &dyn BlockDevice) -> ExofsResult<usize> {
    let size = device.block_size() as usize;
    if size == 0 {
        return Err(ExofsError::InvalidArgument);
    }
    Ok(size)
}

fn block_geometry(
    device: &dyn BlockDevice,
    byte_offset: usize,
) -> ExofsResult<(usize, usize)> {
    let block_size = block_size(device)?;
    Ok((byte_offset / block_size, byte_offset % block_size))
}

/// Lit `out.len()` octets à partir de `base_lba + byte_offset`.
pub fn read_bytes_at(
    device: &dyn BlockDevice,
    base_lba: u64,
    byte_offset: usize,
    out: &mut [u8],
) -> ExofsResult<()> {
    if out.is_empty() {
        return Ok(());
    }

    let (start_block, intra_block_offset) = block_geometry(device, byte_offset)?;
    let block_size = block_size(device)?;
    let mut block = vec![0u8; block_size];
    let mut remaining = out.len();
    let mut dst_offset = 0usize;
    let mut current_lba = base_lba
        .checked_add(start_block as u64)
        .ok_or(ExofsError::OffsetOverflow)?;
    let mut current_intra = intra_block_offset;

    while remaining > 0 {
        device.read_block(current_lba, &mut block)?;
        let available = block_size.saturating_sub(current_intra);
        let chunk_len = remaining.min(available);
        let src_end = current_intra
            .checked_add(chunk_len)
            .ok_or(ExofsError::OffsetOverflow)?;
        let dst_end = dst_offset
            .checked_add(chunk_len)
            .ok_or(ExofsError::OffsetOverflow)?;
        out[dst_offset..dst_end].copy_from_slice(&block[current_intra..src_end]);
        remaining -= chunk_len;
        dst_offset = dst_end;
        current_lba = current_lba
            .checked_add(1)
            .ok_or(ExofsError::OffsetOverflow)?;
        current_intra = 0;
    }

    Ok(())
}

/// Lit `out.len()` octets à partir du début de `base_lba`.
#[inline]
pub fn read_bytes(
    device: &dyn BlockDevice,
    base_lba: u64,
    out: &mut [u8],
) -> ExofsResult<()> {
    read_bytes_at(device, base_lba, 0, out)
}

/// Lit un tableau de taille fixe à partir du début de `base_lba`.
pub fn read_array<const N: usize>(
    device: &dyn BlockDevice,
    base_lba: u64,
) -> ExofsResult<[u8; N]> {
    let mut out = [0u8; N];
    read_bytes(device, base_lba, &mut out)?;
    Ok(out)
}

/// Écrit `data.len()` octets à partir de `base_lba + byte_offset`.
///
/// Les blocs partiels utilisent un read/modify/write pour préserver les octets
/// hors plage ciblée.
pub fn write_bytes_at(
    device: &mut dyn BlockDevice,
    base_lba: u64,
    byte_offset: usize,
    data: &[u8],
) -> ExofsResult<()> {
    if data.is_empty() {
        return Ok(());
    }

    let (start_block, intra_block_offset) = block_geometry(device, byte_offset)?;
    let block_size = block_size(device)?;
    let mut block = vec![0u8; block_size];
    let mut remaining = data.len();
    let mut src_offset = 0usize;
    let mut current_lba = base_lba
        .checked_add(start_block as u64)
        .ok_or(ExofsError::OffsetOverflow)?;
    let mut current_intra = intra_block_offset;

    while remaining > 0 {
        let available = block_size.saturating_sub(current_intra);
        let chunk_len = remaining.min(available);
        if current_intra != 0 || chunk_len != block_size {
            if device.read_block(current_lba, &mut block).is_err() {
                block.fill(0);
            }
        }
        let src_end = src_offset
            .checked_add(chunk_len)
            .ok_or(ExofsError::OffsetOverflow)?;
        let dst_end = current_intra
            .checked_add(chunk_len)
            .ok_or(ExofsError::OffsetOverflow)?;
        block[current_intra..dst_end].copy_from_slice(&data[src_offset..src_end]);
        device.write_block(current_lba, &block)?;
        remaining -= chunk_len;
        src_offset = src_end;
        current_lba = current_lba
            .checked_add(1)
            .ok_or(ExofsError::OffsetOverflow)?;
        current_intra = 0;
    }

    Ok(())
}

/// Écrit `data.len()` octets à partir du début de `base_lba`.
#[inline]
pub fn write_bytes(
    device: &mut dyn BlockDevice,
    base_lba: u64,
    data: &[u8],
) -> ExofsResult<()> {
    write_bytes_at(device, base_lba, 0, data)
}

/// Écrit un tableau de taille fixe à partir du début de `base_lba`.
#[inline]
pub fn write_array<const N: usize>(
    device: &mut dyn BlockDevice,
    base_lba: u64,
    data: &[u8; N],
) -> ExofsResult<()> {
    write_bytes(device, base_lba, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    use alloc::vec::Vec;
    use spin::Mutex;

    struct MockBlockDevice {
        storage: Mutex<Vec<u8>>,
        block_size: u32,
    }

    impl MockBlockDevice {
        fn new(block_size: u32, blocks: usize) -> Self {
            Self {
                storage: Mutex::new(vec![0u8; block_size as usize * blocks]),
                block_size,
            }
        }
    }

    impl BlockDevice for MockBlockDevice {
        fn read_block(&self, lba: u64, buf: &mut [u8]) -> ExofsResult<()> {
            let block_size = self.block_size as usize;
            if buf.len() != block_size {
                return Err(ExofsError::InvalidArgument);
            }
            let start = lba as usize * block_size;
            let end = start + block_size;
            let storage = self.storage.lock();
            if end > storage.len() {
                return Err(ExofsError::IoError);
            }
            buf.copy_from_slice(&storage[start..end]);
            Ok(())
        }

        fn write_block(&self, lba: u64, buf: &[u8]) -> ExofsResult<()> {
            let block_size = self.block_size as usize;
            if buf.len() != block_size {
                return Err(ExofsError::InvalidArgument);
            }
            let start = lba as usize * block_size;
            let end = start + block_size;
            let mut storage = self.storage.lock();
            if end > storage.len() {
                return Err(ExofsError::IoError);
            }
            storage[start..end].copy_from_slice(buf);
            Ok(())
        }

        fn block_size(&self) -> u32 {
            self.block_size
        }

        fn total_blocks(&self) -> u64 {
            self.storage.lock().len() as u64 / self.block_size as u64
        }

        fn flush(&self) -> ExofsResult<()> {
            Ok(())
        }
    }

    #[test]
    fn test_read_write_bytes_across_partial_blocks() {
        let mut device = MockBlockDevice::new(16, 4);
        let payload = *b"abcdefghijklmnopqrstuvwxyz";
        write_bytes_at(&mut device, 0, 6, &payload).unwrap();

        let mut out = [0u8; 26];
        read_bytes_at(&device, 0, 6, &mut out).unwrap();
        assert_eq!(out, payload);
    }

    #[test]
    fn test_block_io_stress_roundtrips() {
        let mut device = MockBlockDevice::new(64, 128);
        let mut seed = 0x1234_5678_9ABC_DEF0u64;

        let mut round = 0usize;
        while round < 256 {
            let offset = (round * 13) % 512;
            let len = 1 + ((round * 29) % 190);
            let mut payload = vec![0u8; len];
            let mut idx = 0usize;
            while idx < payload.len() {
                seed = seed.rotate_left(7).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                payload[idx] = (seed & 0xFF) as u8;
                idx = idx.wrapping_add(1);
            }

            write_bytes_at(&mut device, 2, offset, &payload).unwrap();

            let mut out = vec![0u8; len];
            read_bytes_at(&device, 2, offset, &mut out).unwrap();
            assert_eq!(out, payload);
            round = round.wrapping_add(1);
        }
    }
}
