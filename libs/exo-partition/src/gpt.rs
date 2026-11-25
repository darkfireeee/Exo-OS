//! Parsing de la table GPT (GUID Partition Table) — header + entrées, avec
//! validation **réelle** de la signature et des CRC-32 (header + table).

use crate::crc32::crc32;
use crate::guid::Guid;

/// Signature du header GPT.
pub const GPT_SIGNATURE: &[u8; 8] = b"EFI PART";
/// Taille minimale d'un header GPT (rev 1.0).
pub const GPT_HEADER_MIN_SIZE: usize = 92;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GptError {
    BadSignature,
    BadHeaderSize,
    HeaderCrcMismatch,
    TableCrcMismatch,
    BufferTooSmall,
    NoEntries,
}

#[inline]
fn rd_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
#[inline]
fn rd_u64(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([
        b[off], b[off + 1], b[off + 2], b[off + 3], b[off + 4], b[off + 5], b[off + 6], b[off + 7],
    ])
}

/// Header GPT décodé (champs utiles).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GptHeader {
    pub revision: u32,
    pub header_size: u32,
    pub header_crc32: u32,
    pub my_lba: u64,
    pub alternate_lba: u64,
    pub first_usable_lba: u64,
    pub last_usable_lba: u64,
    pub disk_guid: Guid,
    pub partition_entry_lba: u64,
    pub num_partition_entries: u32,
    pub sizeof_partition_entry: u32,
    pub partition_table_crc32: u32,
}

impl GptHeader {
    /// Parse + valide la signature et le **CRC du header**. `block` doit contenir
    /// le bloc LBA 1 (≥ header_size octets).
    pub fn parse(block: &[u8]) -> Result<Self, GptError> {
        if block.len() < GPT_HEADER_MIN_SIZE {
            return Err(GptError::BufferTooSmall);
        }
        if &block[0..8] != GPT_SIGNATURE {
            return Err(GptError::BadSignature);
        }
        let header_size = rd_u32(block, 12) as usize;
        if header_size < GPT_HEADER_MIN_SIZE || header_size > block.len() {
            return Err(GptError::BadHeaderSize);
        }
        let stored_crc = rd_u32(block, 16);

        // CRC du header : champ header_crc32 (offset 16..20) mis à zéro.
        let mut hdr = [0u8; 512];
        if header_size > hdr.len() {
            return Err(GptError::BadHeaderSize);
        }
        hdr[..header_size].copy_from_slice(&block[..header_size]);
        hdr[16] = 0;
        hdr[17] = 0;
        hdr[18] = 0;
        hdr[19] = 0;
        if crc32(&hdr[..header_size]) != stored_crc {
            return Err(GptError::HeaderCrcMismatch);
        }

        let mut disk_guid = [0u8; 16];
        disk_guid.copy_from_slice(&block[56..72]);

        Ok(Self {
            revision: rd_u32(block, 8),
            header_size: header_size as u32,
            header_crc32: stored_crc,
            my_lba: rd_u64(block, 24),
            alternate_lba: rd_u64(block, 32),
            first_usable_lba: rd_u64(block, 40),
            last_usable_lba: rd_u64(block, 48),
            disk_guid: Guid(disk_guid),
            partition_entry_lba: rd_u64(block, 72),
            num_partition_entries: rd_u32(block, 80),
            sizeof_partition_entry: rd_u32(block, 84),
            partition_table_crc32: rd_u32(block, 88),
        })
    }

    /// Valide le CRC de la table de partitions (octets bruts du tableau d'entrées).
    pub fn validate_table_crc(&self, table: &[u8]) -> Result<(), GptError> {
        let needed = (self.num_partition_entries as usize)
            .checked_mul(self.sizeof_partition_entry as usize)
            .ok_or(GptError::BufferTooSmall)?;
        if table.len() < needed {
            return Err(GptError::BufferTooSmall);
        }
        if crc32(&table[..needed]) != self.partition_table_crc32 {
            return Err(GptError::TableCrcMismatch);
        }
        Ok(())
    }
}

/// Entrée de partition GPT décodée.
#[derive(Clone, Copy, Debug)]
pub struct GptPartitionEntry {
    pub type_guid: Guid,
    pub unique_guid: Guid,
    pub start_lba: u64,
    pub end_lba: u64,
    pub attributes: u64,
}

impl GptPartitionEntry {
    /// Parse une entrée depuis ses octets (≥ 128). Une entrée dont le type-GUID
    /// est nul est une entrée **vide**.
    pub fn parse(b: &[u8]) -> Result<Self, GptError> {
        if b.len() < 128 {
            return Err(GptError::BufferTooSmall);
        }
        let mut type_guid = [0u8; 16];
        type_guid.copy_from_slice(&b[0..16]);
        let mut unique_guid = [0u8; 16];
        unique_guid.copy_from_slice(&b[16..32]);
        Ok(Self {
            type_guid: Guid(type_guid),
            unique_guid: Guid(unique_guid),
            start_lba: rd_u64(b, 32),
            end_lba: rd_u64(b, 40),
            attributes: rd_u64(b, 48),
        })
    }

    #[inline]
    pub fn is_used(&self) -> bool {
        !self.type_guid.is_nil()
    }

    /// Nombre de secteurs (end inclus).
    #[inline]
    pub fn sectors(&self) -> u64 {
        self.end_lba.saturating_sub(self.start_lba).saturating_add(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guid::GUID_ESP;
    extern crate alloc;
    use alloc::vec;
    use alloc::vec::Vec;

    /// Construit un header GPT valide (CRC corrects) + une table d'1 entrée ESP.
    fn build_gpt() -> (Vec<u8>, Vec<u8>) {
        // Table : 1 entrée ESP de 128 o (le reste à 0, ici 4 entrées pour réalisme).
        let entry_size = 128usize;
        let num_entries = 4u32;
        let mut table = vec![0u8; entry_size * num_entries as usize];
        table[0..16].copy_from_slice(&GUID_ESP.0); // type
        table[16..32].copy_from_slice(&[0xAB; 16]); // unique
        table[32..40].copy_from_slice(&2048u64.to_le_bytes()); // start
        table[40..48].copy_from_slice(&4095u64.to_le_bytes()); // end
        let table_crc = crc32(&table);

        // Header.
        let mut hdr = vec![0u8; 512];
        hdr[0..8].copy_from_slice(GPT_SIGNATURE);
        hdr[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes()); // rev 1.0
        hdr[12..16].copy_from_slice(&92u32.to_le_bytes()); // header_size
        // header_crc32 (16..20) = 0 pour le calcul
        hdr[24..32].copy_from_slice(&1u64.to_le_bytes()); // my_lba
        hdr[32..40].copy_from_slice(&0xFFFFu64.to_le_bytes()); // alternate
        hdr[40..48].copy_from_slice(&34u64.to_le_bytes()); // first usable
        hdr[48..56].copy_from_slice(&0xFF00u64.to_le_bytes()); // last usable
        hdr[56..72].copy_from_slice(&[0xCD; 16]); // disk guid
        hdr[72..80].copy_from_slice(&2u64.to_le_bytes()); // partition_entry_lba
        hdr[80..84].copy_from_slice(&num_entries.to_le_bytes());
        hdr[84..88].copy_from_slice(&(entry_size as u32).to_le_bytes());
        hdr[88..92].copy_from_slice(&table_crc.to_le_bytes());
        let hcrc = crc32(&hdr[..92]);
        hdr[16..20].copy_from_slice(&hcrc.to_le_bytes());
        (hdr, table)
    }

    #[test]
    fn parse_valid_header_and_table() {
        let (hdr, table) = build_gpt();
        let h = GptHeader::parse(&hdr).expect("header valide");
        assert_eq!(h.num_partition_entries, 4);
        assert_eq!(h.sizeof_partition_entry, 128);
        assert_eq!(h.partition_entry_lba, 2);
        h.validate_table_crc(&table).expect("CRC table OK");

        let e0 = GptPartitionEntry::parse(&table[0..128]).unwrap();
        assert!(e0.is_used());
        assert_eq!(e0.type_guid, GUID_ESP);
        assert_eq!(e0.start_lba, 2048);
        assert_eq!(e0.sectors(), 2048);
        // Entrée 1 vide.
        let e1 = GptPartitionEntry::parse(&table[128..256]).unwrap();
        assert!(!e1.is_used());
    }

    #[test]
    fn bad_signature_rejected() {
        let (mut hdr, _) = build_gpt();
        hdr[0] = b'X';
        assert_eq!(GptHeader::parse(&hdr), Err(GptError::BadSignature));
    }

    #[test]
    fn corrupt_header_crc_rejected() {
        let (mut hdr, _) = build_gpt();
        hdr[24] ^= 0xFF; // altère my_lba sans recalculer le CRC
        assert_eq!(GptHeader::parse(&hdr), Err(GptError::HeaderCrcMismatch));
    }

    #[test]
    fn corrupt_table_crc_rejected() {
        let (hdr, mut table) = build_gpt();
        let h = GptHeader::parse(&hdr).unwrap();
        table[33] ^= 0xFF; // altère une entrée
        assert_eq!(h.validate_table_crc(&table), Err(GptError::TableCrcMismatch));
    }
}
