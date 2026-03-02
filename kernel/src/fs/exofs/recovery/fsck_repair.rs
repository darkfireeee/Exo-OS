//! FsckRepair — réparations : orphelins → lost+found, tronqués → truncate ExoFS.

use crate::fs::exofs::core::FsError;
use super::boot_recovery::BlockDevice;

/// LBA du répertoire lost+found (réservé en début de filesystem).
pub const LOST_FOUND_LBA: u64 = 0x50;

/// Entrée lost+found on-disk.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LostFoundEntry {
    pub magic:   u32,
    pub lba:     u64,
    pub size:    u32,
    pub _pad:    [u8; 4],
}

const _: () = assert!(core::mem::size_of::<LostFoundEntry>() == 24);
pub const LOST_FOUND_MAGIC: u32 = 0x4C4F5354; // "LOST"

pub struct FsckRepair;

impl FsckRepair {
    /// Déplace un blob orphelin vers lost+found en écrivant son adresse.
    pub fn move_to_lost_found(device: &mut dyn BlockDevice, blob_lba: u64) -> Result<(), FsError> {
        let entry = LostFoundEntry {
            magic: LOST_FOUND_MAGIC,
            lba:   blob_lba,
            size:  0,
            _pad:  [0; 4],
        };
        // SAFETY: LostFoundEntry repr(C) 24B.
        let buf: [u8; 24] = unsafe { core::mem::transmute_copy(&entry) };
        let mut full_buf = [0u8; 512];
        full_buf[..24].copy_from_slice(&buf);
        // Écrire dans la zone lost+found (LBA simple pour ce module).
        device.write_block(LOST_FOUND_LBA, &full_buf)?;
        Ok(())
    }

    /// Tronque un blob corrompu à une taille sûre.
    pub fn truncate_blob(
        device:   &mut dyn BlockDevice,
        blob_lba: u64,
        new_size: u64,
    ) -> Result<(), FsError> {
        // Lire l'en-tête du blob.
        let mut buf = [0u8; 96];
        device.read_block(blob_lba, &mut buf)?;

        // Réécrire la taille tronquée.
        let new_size_bytes = new_size.to_le_bytes();
        buf[40..48].copy_from_slice(&new_size_bytes);

        // Invalider l'ancien checksum.
        buf[80..88].copy_from_slice(&[0u8; 8]);

        device.write_block(blob_lba, &buf)?;
        Ok(())
    }

    /// Marque un bloc comme supprimé en effaçant le magic.
    pub fn zero_magic(device: &mut dyn BlockDevice, lba: u64) -> Result<(), FsError> {
        let mut buf = [0u8; 96];
        device.read_block(lba, &mut buf)?;
        buf[0..8].copy_from_slice(&[0u8; 8]); // Effacer magic.
        device.write_block(lba, &buf)?;
        Ok(())
    }
}
