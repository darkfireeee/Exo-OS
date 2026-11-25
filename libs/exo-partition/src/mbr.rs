//! Parsing MBR (Master Boot Record) — table de partitions legacy + détection du
//! MBR protecteur GPT (type 0xEE).

/// Type de partition "protective MBR" (indique un disque GPT).
pub const MBR_TYPE_GPT_PROTECTIVE: u8 = 0xEE;
/// Signature de fin de MBR.
pub const MBR_SIGNATURE: u16 = 0xAA55;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MbrError {
    BufferTooSmall,
    BadSignature,
}

#[inline]
fn rd_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// Une entrée de partition MBR (16 octets).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MbrPartitionEntry {
    pub bootable: bool,
    pub part_type: u8,
    pub start_lba: u32,
    pub num_sectors: u32,
}

impl MbrPartitionEntry {
    #[inline]
    pub fn is_used(&self) -> bool {
        self.part_type != 0 && self.num_sectors != 0
    }
}

/// MBR décodé : 4 entrées primaires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mbr {
    pub entries: [MbrPartitionEntry; 4],
}

impl Mbr {
    /// Parse le bloc LBA 0 (≥ 512 octets). Vérifie la signature 0xAA55.
    pub fn parse(block: &[u8]) -> Result<Self, MbrError> {
        if block.len() < 512 {
            return Err(MbrError::BufferTooSmall);
        }
        let sig = u16::from_le_bytes([block[510], block[511]]);
        if sig != MBR_SIGNATURE {
            return Err(MbrError::BadSignature);
        }
        let mut entries = [MbrPartitionEntry {
            bootable: false,
            part_type: 0,
            start_lba: 0,
            num_sectors: 0,
        }; 4];
        let mut i = 0;
        while i < 4 {
            let off = 446 + i * 16;
            entries[i] = MbrPartitionEntry {
                bootable: block[off] == 0x80,
                part_type: block[off + 4],
                start_lba: rd_u32(block, off + 8),
                num_sectors: rd_u32(block, off + 12),
            };
            i += 1;
        }
        Ok(Self { entries })
    }

    /// Vrai si le MBR est un **MBR protecteur GPT** (une entrée de type 0xEE) :
    /// dans ce cas il faut parser la GPT, pas les entrées MBR.
    pub fn is_protective_gpt(&self) -> bool {
        self.entries.iter().any(|e| e.part_type == MBR_TYPE_GPT_PROTECTIVE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate alloc;
    use alloc::vec;

    #[test]
    fn parse_classic_mbr() {
        let mut blk = vec![0u8; 512];
        // Entrée 0 : bootable, type 0x83 (Linux), start 2048, 1 MiB.
        blk[446] = 0x80;
        blk[446 + 4] = 0x83;
        blk[446 + 8..446 + 12].copy_from_slice(&2048u32.to_le_bytes());
        blk[446 + 12..446 + 16].copy_from_slice(&2048u32.to_le_bytes());
        blk[510] = 0x55;
        blk[511] = 0xAA;
        let mbr = Mbr::parse(&blk).unwrap();
        assert!(mbr.entries[0].is_used());
        assert!(mbr.entries[0].bootable);
        assert_eq!(mbr.entries[0].part_type, 0x83);
        assert_eq!(mbr.entries[0].start_lba, 2048);
        assert!(!mbr.entries[1].is_used());
        assert!(!mbr.is_protective_gpt());
    }

    #[test]
    fn detect_protective_gpt() {
        let mut blk = vec![0u8; 512];
        blk[446 + 4] = MBR_TYPE_GPT_PROTECTIVE;
        blk[446 + 12..446 + 16].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        blk[510] = 0x55;
        blk[511] = 0xAA;
        let mbr = Mbr::parse(&blk).unwrap();
        assert!(mbr.is_protective_gpt());
    }

    #[test]
    fn bad_signature_rejected() {
        let blk = vec![0u8; 512];
        assert_eq!(Mbr::parse(&blk), Err(MbrError::BadSignature));
    }
}
