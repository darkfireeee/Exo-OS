#![no_std]
//! exo-partition — parsing GPT/MBR **réel** pour Exo-OS (équivalent de
//! redox-os/partitionlib).
//!
//! Source UNIQUE partagée entre le bootloader (`exo-boot`) et le kernel/runtime :
//! plus de LBA codé en dur — on lit la vraie table de partitions, on valide les
//! CRC, et on résout les type-GUID dynamiquement (ESP / ExoFS ROOT / ExoFS DATA).
//!
//! Flux : `scan(reader)` lit LBA 0 → si MBR protecteur GPT, parse la GPT (header
//! primaire, fallback header de **backup** en cas de corruption) ; sinon parse la
//! table MBR legacy.

extern crate alloc;

pub mod crc32;
pub mod gpt;
pub mod guid;
pub mod mbr;

use alloc::vec;
use alloc::vec::Vec;
use gpt::{GptError, GptHeader, GptPartitionEntry};
use guid::{Guid, PartitionType};
use mbr::{Mbr, MbrError};

pub use guid::{GUID_ESP, GUID_EXOOS_DATA, GUID_EXOOS_ROOT};

/// Nombre maximal d'entrées GPT qu'on accepte de lire (garde-fou anti-OOM sur un
/// header corrompu annonçant des millions d'entrées).
const MAX_GPT_ENTRIES: u32 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartError {
    Io,
    Gpt(GptError),
    NoPartitionTable,
    TooManyEntries,
}

impl From<GptError> for PartError {
    fn from(e: GptError) -> Self {
        PartError::Gpt(e)
    }
}

/// Lecteur de blocs abstrait : le bootloader l'implémente sur INT 13h / EFI
/// BlockIo, le kernel sur le driver block (virtio/NVMe/AHCI).
pub trait BlockReader {
    /// Taille d'un secteur logique (512 ou 4096).
    fn block_size(&self) -> usize;
    /// Nombre total de blocs (pour localiser le GPT de backup en fin de disque).
    fn num_blocks(&self) -> u64;
    /// Lit `buf.len()` octets à partir du secteur `lba`.
    fn read_lba(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), PartError>;
}

/// Schéma de partitionnement détecté.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    Gpt,
    Mbr,
}

/// Une partition résolue (unifie GPT et MBR).
#[derive(Clone, Copy, Debug)]
pub struct Partition {
    pub index: usize,
    pub start_lba: u64,
    pub sectors: u64,
    /// Type-GUID (GPT) ou `None` (MBR).
    pub type_guid: Option<Guid>,
    /// Octet de type (MBR) ou `None` (GPT).
    pub mbr_type: Option<u8>,
}

impl Partition {
    /// Catégorie ExoOS résolue (GPT uniquement).
    pub fn part_type(&self) -> PartitionType {
        match self.type_guid {
            Some(g) => g.partition_type(),
            None => PartitionType::Other,
        }
    }
    /// Dernier LBA inclus.
    pub fn end_lba(&self) -> u64 {
        self.start_lba.saturating_add(self.sectors).saturating_sub(1)
    }
}

/// Résultat d'un scan : schéma + partitions utilisées.
pub struct PartitionTable {
    pub scheme: Scheme,
    pub disk_guid: Option<Guid>,
    pub partitions: Vec<Partition>,
}

impl PartitionTable {
    /// Première partition d'une catégorie ExoOS donnée.
    pub fn find(&self, ty: PartitionType) -> Option<&Partition> {
        self.partitions.iter().find(|p| p.part_type() == ty)
    }
    pub fn esp(&self) -> Option<&Partition> {
        self.find(PartitionType::Esp)
    }
    pub fn exofs_root(&self) -> Option<&Partition> {
        self.find(PartitionType::ExoFsRoot)
    }
    pub fn exofs_data(&self) -> Option<&Partition> {
        self.find(PartitionType::ExoFsData)
    }
}

/// Lit + valide un header GPT à un LBA donné.
fn read_gpt_header<R: BlockReader>(r: &mut R, lba: u64, bs: usize) -> Result<GptHeader, PartError> {
    let mut block = vec![0u8; bs];
    r.read_lba(lba, &mut block)?;
    Ok(GptHeader::parse(&block)?)
}

/// Lit la table d'entrées + valide son CRC + parse les entrées utilisées.
fn read_gpt_entries<R: BlockReader>(
    r: &mut R,
    h: &GptHeader,
    bs: usize,
) -> Result<Vec<Partition>, PartError> {
    if h.num_partition_entries > MAX_GPT_ENTRIES {
        return Err(PartError::TooManyEntries);
    }
    let entry_size = h.sizeof_partition_entry as usize;
    if entry_size < 128 {
        return Err(PartError::Gpt(GptError::BufferTooSmall));
    }
    let total = (h.num_partition_entries as usize)
        .checked_mul(entry_size)
        .ok_or(PartError::TooManyEntries)?;
    let blocks = total.div_ceil(bs);
    let mut table = vec![0u8; blocks * bs];
    let mut i = 0u64;
    while (i as usize) < blocks {
        let off = i as usize * bs;
        r.read_lba(h.partition_entry_lba + i, &mut table[off..off + bs])?;
        i += 1;
    }
    h.validate_table_crc(&table)?;

    let mut out = Vec::new();
    let mut idx = 0usize;
    while idx < h.num_partition_entries as usize {
        let off = idx * entry_size;
        let e = GptPartitionEntry::parse(&table[off..off + 128])?;
        if e.is_used() {
            out.push(Partition {
                index: idx,
                start_lba: e.start_lba,
                sectors: e.sectors(),
                type_guid: Some(e.type_guid),
                mbr_type: None,
            });
        }
        idx += 1;
    }
    Ok(out)
}

/// Scanne la table de partitions. GPT (avec fallback backup) sinon MBR legacy.
pub fn scan<R: BlockReader>(r: &mut R) -> Result<PartitionTable, PartError> {
    let bs = r.block_size();
    if bs < 512 {
        return Err(PartError::NoPartitionTable);
    }

    // LBA 0 : MBR (protecteur GPT ou legacy).
    let mut lba0 = vec![0u8; bs];
    r.read_lba(0, &mut lba0)?;
    let mbr = match Mbr::parse(&lba0) {
        Ok(m) => m,
        Err(MbrError::BadSignature) => return Err(PartError::NoPartitionTable),
        Err(_) => return Err(PartError::NoPartitionTable),
    };

    if mbr.is_protective_gpt() {
        // GPT : header primaire (LBA 1), fallback header de backup (dernier LBA).
        let header = match read_gpt_header(r, 1, bs) {
            Ok(h) => h,
            Err(_) => {
                let last = r.num_blocks().saturating_sub(1);
                read_gpt_header(r, last, bs)?
            }
        };
        let partitions = read_gpt_entries(r, &header, bs)?;
        return Ok(PartitionTable {
            scheme: Scheme::Gpt,
            disk_guid: Some(header.disk_guid),
            partitions,
        });
    }

    // MBR legacy.
    let mut partitions = Vec::new();
    for (i, e) in mbr.entries.iter().enumerate() {
        if e.is_used() {
            partitions.push(Partition {
                index: i,
                start_lba: e.start_lba as u64,
                sectors: e.num_sectors as u64,
                type_guid: None,
                mbr_type: Some(e.part_type),
            });
        }
    }
    if partitions.is_empty() {
        return Err(PartError::NoPartitionTable);
    }
    Ok(PartitionTable {
        scheme: Scheme::Mbr,
        disk_guid: None,
        partitions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crc32::crc32;
    use crate::gpt::GPT_SIGNATURE;
    use crate::mbr::{MBR_SIGNATURE, MBR_TYPE_GPT_PROTECTIVE};

    const BS: usize = 512;

    /// Disque synthétique en mémoire implémentant BlockReader.
    struct MemDisk {
        data: Vec<u8>,
    }
    impl MemDisk {
        fn new(blocks: usize) -> Self {
            Self {
                data: vec![0u8; blocks * BS],
            }
        }
        fn put(&mut self, lba: u64, off: usize, bytes: &[u8]) {
            let base = lba as usize * BS + off;
            self.data[base..base + bytes.len()].copy_from_slice(bytes);
        }
    }
    impl BlockReader for MemDisk {
        fn block_size(&self) -> usize {
            BS
        }
        fn num_blocks(&self) -> u64 {
            (self.data.len() / BS) as u64
        }
        fn read_lba(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), PartError> {
            let base = lba as usize * BS;
            if base + buf.len() > self.data.len() {
                return Err(PartError::Io);
            }
            buf.copy_from_slice(&self.data[base..base + buf.len()]);
            Ok(())
        }
    }

    /// Construit un disque GPT complet : MBR protecteur + header + table (ESP,
    /// ROOT, DATA), avec CRC corrects.
    fn build_gpt_disk() -> MemDisk {
        let mut disk = MemDisk::new(128);
        // MBR protecteur (LBA 0).
        disk.put(0, 446 + 4, &[MBR_TYPE_GPT_PROTECTIVE]);
        disk.put(0, 446 + 12, &0xFFFF_FFFFu32.to_le_bytes());
        disk.put(0, 510, &MBR_SIGNATURE.to_le_bytes());

        // Table de partitions (LBA 2..), 4 entrées de 128 o.
        let entry_size = 128usize;
        let num_entries = 4u32;
        let mut table = vec![0u8; entry_size * num_entries as usize];
        let mut mk = |slot: usize, ty: &Guid, start: u64, end: u64| {
            let o = slot * entry_size;
            table[o..o + 16].copy_from_slice(&ty.0);
            table[o + 16..o + 32].copy_from_slice(&[(slot as u8) + 1; 16]);
            table[o + 32..o + 40].copy_from_slice(&start.to_le_bytes());
            table[o + 40..o + 48].copy_from_slice(&end.to_le_bytes());
        };
        mk(0, &GUID_ESP, 2048, 2048 + 524288 - 1); // ESP 256 MiB
        mk(1, &GUID_EXOOS_ROOT, 526336, 526336 + 8388608 - 1); // ROOT 4 GiB
        mk(2, &GUID_EXOOS_DATA, 8914944, 8914944 + 1048576 - 1); // DATA
        let table_crc = crc32(&table);

        // Header (LBA 1).
        let mut hdr = vec![0u8; 92];
        hdr[0..8].copy_from_slice(GPT_SIGNATURE);
        hdr[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        hdr[12..16].copy_from_slice(&92u32.to_le_bytes());
        hdr[24..32].copy_from_slice(&1u64.to_le_bytes());
        hdr[32..40].copy_from_slice(&127u64.to_le_bytes());
        hdr[40..48].copy_from_slice(&34u64.to_le_bytes());
        hdr[48..56].copy_from_slice(&93u64.to_le_bytes());
        hdr[56..72].copy_from_slice(&[0xEE; 16]);
        hdr[72..80].copy_from_slice(&2u64.to_le_bytes());
        hdr[80..84].copy_from_slice(&num_entries.to_le_bytes());
        hdr[84..88].copy_from_slice(&(entry_size as u32).to_le_bytes());
        hdr[88..92].copy_from_slice(&table_crc.to_le_bytes());
        let hcrc = crc32(&hdr[..92]);
        hdr[16..20].copy_from_slice(&hcrc.to_le_bytes());

        disk.put(1, 0, &hdr);
        // Table sur LBA 2.. (32 blocs pour 4×128=512 o → 1 bloc suffit ici).
        disk.put(2, 0, &table);
        disk
    }

    #[test]
    fn scan_gpt_finds_all_three_partitions() {
        let mut disk = build_gpt_disk();
        let table = scan(&mut disk).expect("scan GPT");
        assert_eq!(table.scheme, Scheme::Gpt);
        assert_eq!(table.partitions.len(), 3);

        let esp = table.esp().expect("ESP");
        assert_eq!(esp.start_lba, 2048);
        assert_eq!(esp.part_type(), PartitionType::Esp);

        let root = table.exofs_root().expect("ROOT");
        assert_eq!(root.start_lba, 526336);
        assert_eq!(root.part_type(), PartitionType::ExoFsRoot);

        let data = table.exofs_data().expect("DATA");
        assert_eq!(data.part_type(), PartitionType::ExoFsData);
    }

    #[test]
    fn scan_corrupt_primary_header_falls_back_to_backup() {
        let mut disk = build_gpt_disk();
        // Recopier le header valide au dernier LBA (backup), puis corrompre le primaire.
        let last = disk.num_blocks() - 1;
        let mut hdr = vec![0u8; BS];
        disk.read_lba(1, &mut hdr).unwrap();
        disk.put(last, 0, &hdr[..92]);
        disk.put(1, 0, &[0xFF; 8]); // casse la signature primaire

        let table = scan(&mut disk).expect("fallback backup GPT");
        assert_eq!(table.partitions.len(), 3);
        assert!(table.esp().is_some());
    }

    #[test]
    fn scan_legacy_mbr() {
        let mut disk = MemDisk::new(64);
        disk.put(0, 446, &[0x80]); // bootable
        disk.put(0, 446 + 4, &[0x83]); // Linux
        disk.put(0, 446 + 8, &2048u32.to_le_bytes());
        disk.put(0, 446 + 12, &4096u32.to_le_bytes());
        disk.put(0, 510, &MBR_SIGNATURE.to_le_bytes());
        let table = scan(&mut disk).expect("scan MBR");
        assert_eq!(table.scheme, Scheme::Mbr);
        assert_eq!(table.partitions.len(), 1);
        assert_eq!(table.partitions[0].mbr_type, Some(0x83));
        assert_eq!(table.partitions[0].start_lba, 2048);
    }

    #[test]
    fn scan_no_table() {
        let mut disk = MemDisk::new(16); // tout à zéro → pas de signature
        assert_eq!(scan(&mut disk).err(), Some(PartError::NoPartitionTable));
    }
}
